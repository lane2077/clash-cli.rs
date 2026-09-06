use std::env;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tar::Archive;

use crate::auto_sudo;
use crate::cli::{MirrorSource, UpdateCommand};
use crate::http::{build_http_client, download_candidates, download_to_file};
use crate::output::{is_json_mode, print_json};
use crate::utils;

const CLI_REPO: &str = "lane2077/clash-cli.rs";
const CLI_RELEASES_LATEST_API: &str =
    "https://api.github.com/repos/lane2077/clash-cli.rs/releases/latest";

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

pub fn run(command: UpdateCommand) -> Result<()> {
    match command {
        UpdateCommand::Run(args) => cmd_update(args.mirror, args.sha256),
        UpdateCommand::Check(args) => cmd_check(args.mirror),
    }
}

fn cmd_check(mirror: MirrorSource) -> Result<()> {
    let current = current_version();
    let client = build_http_client()?;
    let release = fetch_latest_release(&client)?;
    let latest = &release.tag_name;
    let is_latest = normalize_version(&current) == normalize_version(latest);

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "update.check",
            "current_version": current,
            "latest_version": latest,
            "is_latest": is_latest,
        }));
    }

    println!("当前版本: {}", current);
    println!("最新版本: {}", latest);
    if is_latest {
        println!("已是最新版本。");
    } else {
        let _ = mirror;
        println!("可执行 `clash update run` 升级到最新版本。");
    }
    Ok(())
}

fn cmd_update(mirror: MirrorSource, expected_sha256: Option<String>) -> Result<()> {
    let current_exe = env::current_exe().context("获取当前可执行文件路径失败")?;

    if let Some(v) = expected_sha256.as_ref()
        && !is_valid_sha256(v)
    {
        bail!("SHA256 参数格式不合法: {v}");
    }

    // 检查是否需要 sudo
    if needs_sudo(&current_exe) && auto_sudo::should_auto_delegate(is_json_mode()) {
        if !is_json_mode() {
            println!("检测到权限不足，正在请求 sudo 授权继续执行 update ...");
        }
        let status = auto_sudo::run_with_sudo(is_json_mode(), |cmd| {
            cmd.arg("update").arg("run");
            cmd.arg("--mirror").arg(mirror_str(mirror));
            if let Some(v) = &expected_sha256 {
                cmd.arg("--sha256").arg(v);
            }
            Ok(())
        })?;
        if status.success() {
            return Ok(());
        }
        bail!("sudo 授权未通过或命令执行失败，请手动使用 sudo 重试");
    }

    let current = current_version();
    let client = build_http_client()?;
    let release = fetch_latest_release(&client)?;
    let latest = &release.tag_name;

    if normalize_version(&current) == normalize_version(latest) {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "update.run",
                "current_version": current,
                "latest_version": latest,
                "updated": false,
                "reason": "already latest",
            }));
        }
        println!("已是最新版本: {}", current);
        return Ok(());
    }

    let asset = select_cli_asset(&release.assets)?;
    let candidate_urls = download_candidates(&asset.browser_download_url, mirror);
    let checksum = match expected_sha256 {
        Some(value) => value,
        None => {
            let trusted =
                if let Some(digest) = asset.digest.as_deref().and_then(asset_digest_sha256) {
                    Some(digest.to_string())
                } else {
                    parse_checksum_from_release_assets(&release.assets, &asset.name)?
                };
            trusted
                .with_context(|| format!("发布资产 {} 缺少可信 SHA256，已拒绝自更新", asset.name))?
        }
    };

    let tmp_archive = current_exe.with_extension("update.tar.gz");
    let mut chosen_url = None;
    let mut errors = Vec::new();
    for url in candidate_urls {
        match download_to_file(&client, &url, &tmp_archive) {
            Ok(()) => {
                chosen_url = Some(url);
                break;
            }
            Err(err) => errors.push(format!("{url} => {err}")),
        }
    }

    let source_url = match chosen_url {
        Some(url) => url,
        None => {
            let _ = fs::remove_file(&tmp_archive);
            bail!("下载失败，已尝试所有源:\n{}", errors.join("\n"));
        }
    };

    let actual_sha256 = sha256_hex(&tmp_archive)?;
    if !compare_sha256(&actual_sha256, &checksum) {
        let _ = fs::remove_file(&tmp_archive);
        bail!("SHA256 校验失败: expected={checksum}, actual={actual_sha256}");
    }

    let tmp_dir = current_exe.with_extension("update_tmp");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).context("创建临时解压目录失败")?;
    if let Err(err) = extract_archive(&tmp_archive, &tmp_dir) {
        let _ = fs::remove_file(&tmp_archive);
        let _ = fs::remove_dir_all(&tmp_dir);
        bail!("解压失败: {err}");
    }

    let new_binary = find_extracted_binary(&tmp_dir)?;

    // 设置权限并替换
    fs::set_permissions(&new_binary, fs::Permissions::from_mode(0o755))
        .context("设置执行权限失败")?;

    let tmp_new = current_exe.with_extension("new");
    fs::copy(&new_binary, &tmp_new).context("复制新二进制失败")?;
    fs::set_permissions(&tmp_new, fs::Permissions::from_mode(0o755)).context("设置执行权限失败")?;
    fs::rename(&tmp_new, &current_exe).context("替换当前二进制失败")?;

    // 清理临时文件
    let _ = fs::remove_file(&tmp_archive);
    let _ = fs::remove_dir_all(&tmp_dir);

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "update.run",
            "current_version": current,
            "latest_version": latest,
            "updated": true,
            "asset": asset.name,
            "source": source_url,
        }));
    }

    println!("更新完成: {} -> {}", current, latest);
    println!("下载来源: {}", source_url);
    Ok(())
}

fn current_version() -> String {
    format!("v{}", env!("CARGO_PKG_VERSION"))
}

fn normalize_version(v: &str) -> String {
    v.trim_start_matches('v').to_string()
}

fn fetch_latest_release(client: &reqwest::blocking::Client) -> Result<GitHubRelease> {
    let response = client
        .get(CLI_RELEASES_LATEST_API)
        .send()
        .with_context(|| format!("请求 {} 发布信息失败", CLI_REPO))?
        .error_for_status()
        .with_context(|| format!("{} 发布信息返回非成功状态", CLI_REPO))?;

    response.json::<GitHubRelease>().context("解析发布信息失败")
}

fn host_os_tag() -> Result<&'static str> {
    match env::consts::OS {
        "linux" => Ok("linux"),
        "macos" => Ok("darwin"),
        other => bail!("暂不支持的系统: {other}"),
    }
}

fn select_cli_asset(assets: &[GitHubAsset]) -> Result<GitHubAsset> {
    select_cli_asset_for(host_os_tag()?, env::consts::ARCH, assets)
}

pub(crate) fn select_cli_asset_for(
    os_tag: &str,
    arch: &str,
    assets: &[GitHubAsset],
) -> Result<GitHubAsset> {
    let arch_keyword = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => bail!("暂不支持的架构: {arch}"),
    };
    let os_tag = os_tag.to_lowercase();

    let target = format!("clash-{os_tag}-{arch_keyword}.tar.gz");
    for asset in assets {
        if asset.name.to_lowercase() == target {
            return Ok(asset.clone());
        }
    }

    for asset in assets {
        let name = asset.name.to_lowercase();
        if name.contains(&os_tag) && name.contains(arch_keyword) && name.ends_with(".tar.gz") {
            return Ok(asset.clone());
        }
    }

    bail!("未找到匹配的 CLI 发布资产 (os={os_tag}, arch={arch_keyword})")
}

fn find_extracted_binary(dir: &Path) -> Result<std::path::PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries = fs::read_dir(&d).context("读取解压目录失败")?;
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Some(name) = path.file_name() {
                let name_str = name.to_string_lossy();
                if name_str.starts_with("clash") && !name_str.ends_with(".tar.gz") && path.is_file()
                {
                    return Ok(path);
                }
            }
        }
    }
    bail!("解压后未找到 clash 二进制文件")
}

fn asset_digest_sha256(value: &str) -> Option<&str> {
    value
        .strip_prefix("sha256:")
        .filter(|hash| is_valid_sha256(hash))
}

fn parse_checksum_from_release_assets(
    assets: &[GitHubAsset],
    target_asset_name: &str,
) -> Result<Option<String>> {
    let checksum_candidates: Vec<&GitHubAsset> = assets
        .iter()
        .filter(|a| {
            let lower = a.name.to_lowercase();
            lower.contains("sha256") || lower.contains("checksum")
        })
        .collect();

    for asset in checksum_candidates {
        let checksum_data = fetch_checksum_asset(asset)?;
        for line in checksum_data.lines() {
            let mut parts = line.split_whitespace();
            let hash = parts.next();
            let file = parts.next();
            if let (Some(hash), Some(file)) = (hash, file)
                && file == target_asset_name
                && is_valid_sha256(hash)
            {
                return Ok(Some(hash.to_string()));
            }
        }
    }
    Ok(None)
}

fn fetch_checksum_asset(asset: &GitHubAsset) -> Result<String> {
    let client = build_http_client()?;
    let response = client
        .get(&asset.browser_download_url)
        .send()
        .with_context(|| format!("请求 checksum 资源失败: {}", asset.name))?
        .error_for_status()
        .with_context(|| format!("checksum 资源返回失败: {}", asset.name))?;

    let text = response
        .text()
        .with_context(|| format!("读取 checksum 资源失败: {}", asset.name))?;
    Ok(text)
}

fn is_valid_sha256(v: &str) -> bool {
    let v = v.trim();
    v.len() == 64 && v.chars().all(|c| c.is_ascii_hexdigit())
}

fn compare_sha256(actual: &str, expected: &str) -> bool {
    actual.eq_ignore_ascii_case(expected)
}

fn sha256_hex(path: &Path) -> Result<String> {
    let mut file =
        fs::File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let n = file.read(&mut buf).context("读取文件失败")?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn extract_archive(archive_path: &Path, output_dir: &Path) -> Result<()> {
    let file = fs::File::open(archive_path)
        .with_context(|| format!("打开归档文件失败: {}", archive_path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    archive
        .unpack(output_dir)
        .with_context(|| format!("解压归档失败: {}", archive_path.display()))
}

fn needs_sudo(exe_path: &Path) -> bool {
    if utils::is_root_user() {
        return false;
    }
    // 尝试以写模式打开文件，直接测试是否有写权限
    std::fs::OpenOptions::new()
        .write(true)
        .open(exe_path)
        .is_err()
}

fn mirror_str(m: MirrorSource) -> &'static str {
    match m {
        MirrorSource::Auto => "auto",
        MirrorSource::Ghfast => "ghfast",
        MirrorSource::Github => "github",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GitHubAsset {
        GitHubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.invalid/{name}"),
            digest: None,
        }
    }

    #[test]
    fn darwin_arm64_picks_darwin_not_linux() {
        let assets = vec![
            asset("clash-linux-arm64.tar.gz"),
            asset("clash-darwin-arm64.tar.gz"),
            asset("clash-darwin-amd64.tar.gz"),
        ];
        let picked = select_cli_asset_for("darwin", "aarch64", &assets).unwrap();
        assert_eq!(picked.name, "clash-darwin-arm64.tar.gz");
        assert!(!picked.name.contains("linux"));
    }

    #[test]
    fn darwin_amd64_picks_darwin_amd64() {
        let assets = vec![
            asset("clash-linux-amd64.tar.gz"),
            asset("clash-darwin-amd64.tar.gz"),
            asset("clash-darwin-arm64.tar.gz"),
        ];
        let picked = select_cli_asset_for("darwin", "x86_64", &assets).unwrap();
        assert_eq!(picked.name, "clash-darwin-amd64.tar.gz");
    }

    #[test]
    fn linux_arm64_still_picks_linux() {
        let assets = vec![
            asset("clash-darwin-arm64.tar.gz"),
            asset("clash-linux-arm64.tar.gz"),
        ];
        let picked = select_cli_asset_for("linux", "aarch64", &assets).unwrap();
        assert_eq!(picked.name, "clash-linux-arm64.tar.gz");
        assert!(!picked.name.contains("darwin"));
    }
}
