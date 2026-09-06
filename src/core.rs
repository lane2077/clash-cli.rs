use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::cli::{Amd64Variant, CoreCommand, CoreInstallArgs, CoreUpgradeArgs, MirrorSource};
use crate::http::{build_http_client, download_candidates, download_to_file};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;
use crate::utils::{command_exists, is_root_user, write_atomic_text};

const GITHUB_REPO: &str = "MetaCubeX/mihomo";
const RELEASES_LATEST_API: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
const RELEASES_BY_TAG_API_PREFIX: &str =
    "https://api.github.com/repos/MetaCubeX/mihomo/releases/tags/";

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

#[derive(Debug)]
struct CoreMeta {
    version: String,
}

#[derive(Debug)]
struct CoreInstallRequest {
    version: String,
    mirror: MirrorSource,
    amd64_variant: Amd64Variant,
    force: bool,
}

pub fn run(command: CoreCommand) -> Result<()> {
    match command {
        CoreCommand::Install(args) => cmd_install(args),
        CoreCommand::Upgrade(args) => cmd_upgrade(args),
        CoreCommand::Version => cmd_version(),
        CoreCommand::Path => cmd_path(),
    }
}

fn cmd_install(args: CoreInstallArgs) -> Result<()> {
    crate::utils::ensure_supported_host()?;
    let request = CoreInstallRequest {
        version: args.version,
        mirror: args.mirror,
        amd64_variant: args.amd64_variant,
        force: args.force,
    };
    install_mihomo_core(request)
}

fn cmd_upgrade(args: CoreUpgradeArgs) -> Result<()> {
    crate::utils::ensure_supported_host()?;
    let request = CoreInstallRequest {
        version: "latest".to_string(),
        mirror: args.mirror,
        amd64_variant: args.amd64_variant,
        force: args.force,
    };
    install_mihomo_core(request)
}

fn cmd_version() -> Result<()> {
    let paths = app_paths()?;
    if !paths.core_meta_file.exists() {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "core.version",
                "installed": false,
                "version": null
            }));
        }
        println!("内核状态: 未安装");
        return Ok(());
    }
    let meta = load_core_meta(&paths.core_meta_file)?;
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "core.version",
            "installed": true,
            "version": meta.version
        }));
    }
    println!("{}", meta.version);
    Ok(())
}

fn cmd_path() -> Result<()> {
    let paths = app_paths()?;
    if !paths.core_current_link.exists() {
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "core.path",
                "installed": false,
                "path": null
            }));
        }
        println!("内核状态: 未安装");
        return Ok(());
    }
    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "core.path",
            "installed": true,
            "path": paths.core_current_link.display().to_string()
        }));
    }
    println!("{}", paths.core_current_link.display());
    Ok(())
}

fn install_mihomo_core(request: CoreInstallRequest) -> Result<()> {
    let paths = app_paths()?;
    fs::create_dir_all(&paths.core_dir).context("创建内核目录失败")?;
    fs::create_dir_all(&paths.core_versions_dir).context("创建版本目录失败")?;

    let client = build_http_client()?;
    let release = fetch_release(&client, &request.version)?;
    let tag = release.tag_name.clone();
    let asset = select_release_asset(&release.assets, request.amd64_variant)?;

    let version_dir = paths.core_versions_dir.join(&tag);
    let installed_binary = version_dir.join("mihomo");
    fs::create_dir_all(&version_dir).context("创建版本目录失败")?;

    if installed_binary.exists() && !request.force {
        ensure_selinux_executable_label(&installed_binary)?;
        point_current_core(&paths.core_current_link, &installed_binary)?;
        write_core_meta(
            &paths.core_meta_file,
            &tag,
            &asset.name,
            &asset.browser_download_url,
        )?;
        if is_json_mode() {
            return print_json(&serde_json::json!({
                "ok": true,
                "action": "core.install",
                "version": tag,
                "asset": asset.name,
                "path": installed_binary.display().to_string(),
                "source": asset.browser_download_url,
                "reused": true
            }));
        }
        println!("内核已存在: {}", tag);
        println!("当前路径: {}", installed_binary.display());
        return Ok(());
    }

    let candidate_urls = download_candidates(&asset.browser_download_url, request.mirror);
    let temp_gz_path =
        paths
            .core_dir
            .join(format!("mihomo-{}-{}.download.gz", tag, std::process::id()));
    let temp_bin_path = version_dir.join("mihomo.new");

    // 镜像按顺序尝试，确保 ghfast 不可用时自动回退官方源。
    let mut chosen_url = None;
    let mut errors = Vec::new();
    for url in candidate_urls {
        match download_to_file(&client, &url, &temp_gz_path) {
            Ok(()) => {
                chosen_url = Some(url);
                break;
            }
            Err(err) => errors.push(format!("{url} => {err}")),
        }
    }

    let source_url = match chosen_url {
        Some(url) => url,
        None => bail!("下载失败，已尝试所有源:\n{}", errors.join("\n")),
    };

    let expected_sha256 = asset_sha256(&asset)?;
    let actual_sha256 = sha256_hex(&temp_gz_path)?;
    if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        let _ = fs::remove_file(&temp_gz_path);
        bail!("mihomo 下载校验失败: expected={expected_sha256}, actual={actual_sha256}");
    }

    decompress_gzip_to_file(&temp_gz_path, &temp_bin_path)?;
    set_executable(&temp_bin_path)?;

    if installed_binary.exists() {
        fs::remove_file(&installed_binary).context("替换旧内核失败")?;
    }
    fs::rename(&temp_bin_path, &installed_binary).context("落盘新内核失败")?;
    ensure_selinux_executable_label(&installed_binary)?;

    if temp_gz_path.exists() {
        fs::remove_file(&temp_gz_path).ok();
    }

    point_current_core(&paths.core_current_link, &installed_binary)?;
    write_core_meta(&paths.core_meta_file, &tag, &asset.name, &source_url)?;

    if is_json_mode() {
        return print_json(&serde_json::json!({
            "ok": true,
            "action": "core.install",
            "version": tag,
            "asset": asset.name,
            "path": installed_binary.display().to_string(),
            "source": source_url,
            "reused": false
        }));
    }

    println!("内核安装完成: {} ({})", tag, asset.name);
    println!("内核路径: {}", installed_binary.display());
    println!("下载来源: {}", source_url);
    Ok(())
}

/// Fedora 等启用 SELinux 的系统不会把 `/etc` 下的文件自动视为系统程序。
/// systemd 从 `core/mihomo` 软链接启动时，目标文件必须继承正常系统程序
/// 的标签，否则即使 unit 已授予网络能力，TUN 创建仍会被拒绝。
fn ensure_selinux_executable_label(path: &Path) -> Result<()> {
    if std::env::consts::OS != "linux" || !is_root_user() || !command_exists("getenforce") {
        return Ok(());
    }
    let output = Command::new("getenforce")
        .output()
        .context("检测 SELinux 状态失败")?;
    if !output.status.success() || String::from_utf8_lossy(&output.stdout).trim() == "Disabled" {
        return Ok(());
    }
    if !command_exists("chcon") {
        bail!("SELinux 已启用，但系统缺少 chcon，无法设置 Mihomo 可执行文件标签");
    }
    let reference = Path::new("/usr/bin/env");
    if !reference.exists() {
        bail!(
            "SELinux 已启用，但缺少标签参考文件: {}",
            reference.display()
        );
    }
    let status = Command::new("chcon")
        .arg("--reference")
        .arg(reference)
        .arg(path)
        .status()
        .with_context(|| format!("设置 SELinux 可执行文件标签失败: {}", path.display()))?;
    if !status.success() {
        bail!("设置 SELinux 可执行文件标签失败: {}", path.display());
    }
    Ok(())
}

fn fetch_release(client: &reqwest::blocking::Client, version: &str) -> Result<GitHubRelease> {
    let url = if version == "latest" {
        RELEASES_LATEST_API.to_string()
    } else {
        format!("{RELEASES_BY_TAG_API_PREFIX}{version}")
    };

    let response = client
        .get(url.clone())
        .send()
        .with_context(|| format!("请求发布信息失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("发布信息返回非成功状态: {url}"))?;

    response.json::<GitHubRelease>().context("解析发布信息失败")
}

fn host_os_tag() -> Result<&'static str> {
    match env::consts::OS {
        "linux" => Ok("linux"),
        "macos" => Ok("darwin"),
        other => bail!("暂不支持的系统: {other}"),
    }
}

fn select_release_asset(
    assets: &[GitHubAsset],
    amd64_variant: Amd64Variant,
) -> Result<GitHubAsset> {
    select_release_asset_for(host_os_tag()?, env::consts::ARCH, assets, amd64_variant)
}

pub(crate) fn select_release_asset_for(
    os_tag: &str,
    arch: &str,
    assets: &[GitHubAsset],
    amd64_variant: Amd64Variant,
) -> Result<GitHubAsset> {
    let os_tag = os_tag.to_lowercase();
    let mut matched: Vec<GitHubAsset> = assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_lowercase();
            name.contains(&os_tag) && name.ends_with(".gz") && !name.contains("cgo")
        })
        .cloned()
        .collect();

    if matched.is_empty() {
        bail!("{GITHUB_REPO} 当前版本未找到 {os_tag} 资产");
    }

    matched.sort_by(|a, b| a.name.cmp(&b.name));

    match arch {
        "x86_64" => pick_amd64_asset(&matched, amd64_variant),
        "aarch64" => pick_asset_by_keywords(&matched, &["arm64", "aarch64"]),
        "arm" => pick_asset_by_keywords(&matched, &["armv7", "armv6", "arm"]),
        _ => bail!("暂不支持的架构: {arch}"),
    }
}

fn pick_amd64_asset(assets: &[GitHubAsset], variant: Amd64Variant) -> Result<GitHubAsset> {
    let ordered_patterns: &[&str] = match variant {
        Amd64Variant::Auto => &["amd64-compatible", "amd64-v3", "amd64"],
        Amd64Variant::Compatible => &["amd64-compatible", "amd64"],
        Amd64Variant::V3 => &["amd64-v3", "amd64-compatible", "amd64"],
    };

    for pattern in ordered_patterns {
        if let Some(asset) = assets
            .iter()
            .find(|asset| asset.name.to_lowercase().contains(pattern))
        {
            return Ok(asset.clone());
        }
    }

    pick_asset_by_keywords(assets, &["amd64", "x86_64"])
}

fn pick_asset_by_keywords(assets: &[GitHubAsset], keywords: &[&str]) -> Result<GitHubAsset> {
    for keyword in keywords {
        if let Some(asset) = assets
            .iter()
            .find(|asset| asset.name.to_lowercase().contains(keyword))
        {
            return Ok(asset.clone());
        }
    }
    let joined = keywords.join(", ");
    bail!("未找到匹配资产，关键词: {joined}")
}

fn asset_sha256(asset: &GitHubAsset) -> Result<String> {
    let digest = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .filter(|value| value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit()))
        .with_context(|| {
            format!(
                "GitHub 发布资产 {} 缺少可信 SHA256 digest，已拒绝安装",
                asset.name
            )
        })?;
    Ok(digest.to_string())
}

fn sha256_hex(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("打开文件失败: {}", path.display()))?;
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

fn decompress_gzip_to_file(input_gz_path: &Path, output_path: &Path) -> Result<()> {
    let input = File::open(input_gz_path)
        .with_context(|| format!("打开压缩文件失败: {}", input_gz_path.display()))?;
    let mut decoder = GzDecoder::new(input);
    let mut output = File::create(output_path)
        .with_context(|| format!("创建输出文件失败: {}", output_path.display()))?;
    io::copy(&mut decoder, &mut output)
        .with_context(|| format!("解压失败: {}", output_path.display()))?;
    output
        .flush()
        .with_context(|| format!("刷新输出失败: {}", output_path.display()))?;
    Ok(())
}

fn set_executable(path: &Path) -> Result<()> {
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("读取文件属性失败: {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("设置执行权限失败: {}", path.display()))
}

fn point_current_core(current_link: &Path, target: &Path) -> Result<()> {
    if current_link.exists() {
        fs::remove_file(current_link)
            .with_context(|| format!("删除旧链接失败: {}", current_link.display()))?;
    }
    symlink(target, current_link).with_context(|| {
        format!(
            "创建软链接失败: {} -> {}",
            current_link.display(),
            target.display()
        )
    })
}

fn write_core_meta(path: &Path, version: &str, asset_name: &str, source_url: &str) -> Result<()> {
    let installed_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0);
    let content = format!(
        "version={version}\nasset={asset_name}\nsource_url={source_url}\ninstalled_at={installed_at}\n"
    );
    write_atomic_text(path, &content).with_context(|| format!("写入元信息失败: {}", path.display()))
}

fn load_core_meta(path: &Path) -> Result<CoreMeta> {
    let content =
        fs::read_to_string(path).with_context(|| format!("读取元信息失败: {}", path.display()))?;
    let mut version = None;
    for line in content.lines() {
        let mut parts = line.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim();
        if key == "version" {
            version = Some(value.to_string());
        }
    }
    Ok(CoreMeta {
        version: version.context("元信息缺少 version 字段")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str) -> GitHubAsset {
        GitHubAsset {
            name: name.to_string(),
            browser_download_url: format!("https://example.com/{name}"),
            digest: None,
        }
    }

    #[test]
    fn darwin_arm64_picks_darwin_not_linux() {
        let assets = vec![
            asset("mihomo-linux-arm64-v1.19.0.gz"),
            asset("mihomo-linux-amd64-compatible-v1.19.0.gz"),
            asset("mihomo-darwin-arm64-v1.19.0.gz"),
            asset("mihomo-darwin-amd64-v1.19.0.gz"),
        ];
        let picked =
            select_release_asset_for("darwin", "aarch64", &assets, Amd64Variant::Auto).unwrap();
        assert!(picked.name.contains("darwin"), "{}", picked.name);
        assert!(!picked.name.contains("linux"), "{}", picked.name);
        assert!(picked.name.contains("arm64"), "{}", picked.name);
    }

    #[test]
    fn darwin_amd64_picks_darwin_amd64() {
        let assets = vec![
            asset("mihomo-linux-amd64-v1.19.0.gz"),
            asset("mihomo-darwin-amd64-v1.19.0.gz"),
            asset("mihomo-darwin-arm64-v1.19.0.gz"),
        ];
        let picked =
            select_release_asset_for("darwin", "x86_64", &assets, Amd64Variant::Auto).unwrap();
        assert!(picked.name.contains("darwin"));
        assert!(picked.name.contains("amd64"));
        assert!(!picked.name.contains("linux"));
    }

    #[test]
    fn linux_arm64_still_picks_linux() {
        let assets = vec![
            asset("mihomo-linux-arm64-v1.19.0.gz"),
            asset("mihomo-darwin-arm64-v1.19.0.gz"),
        ];
        let picked =
            select_release_asset_for("linux", "aarch64", &assets, Amd64Variant::Auto).unwrap();
        assert!(picked.name.contains("linux"));
        assert!(!picked.name.contains("darwin"));
    }
}
