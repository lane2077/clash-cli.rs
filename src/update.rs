use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

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
struct GitHubAsset {
    name: String,
    browser_download_url: String,
}

pub fn run(command: UpdateCommand) -> Result<()> {
    match command {
        UpdateCommand::Run(args) => cmd_update(args.mirror),
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

fn cmd_update(mirror: MirrorSource) -> Result<()> {
    let current_exe = env::current_exe().context("获取当前可执行文件路径失败")?;

    // 检查是否需要 sudo
    if needs_sudo(&current_exe) {
        if auto_sudo::should_auto_delegate(is_json_mode()) {
            if !is_json_mode() {
                println!("检测到权限不足，正在请求 sudo 授权继续执行 update ...");
            }
            let status = auto_sudo::run_with_sudo(is_json_mode(), |cmd| {
                cmd.arg("update").arg("run");
                cmd.arg("--mirror").arg(mirror_str(mirror));
                Ok(())
            })?;
            if status.success() {
                return Ok(());
            }
            bail!("sudo 授权未通过或命令执行失败，请手动使用 sudo 重试");
        }
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

    // 解压 tar.gz
    let tmp_dir = current_exe.with_extension("update_tmp");
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).context("创建临时解压目录失败")?;

    let output = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tmp_archive)
        .arg("-C")
        .arg(&tmp_dir)
        .output()
        .context("执行 tar 解压失败")?;

    if !output.status.success() {
        let _ = fs::remove_file(&tmp_archive);
        let _ = fs::remove_dir_all(&tmp_dir);
        bail!(
            "解压失败: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
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

fn select_cli_asset(assets: &[GitHubAsset]) -> Result<GitHubAsset> {
    let arch = env::consts::ARCH;
    let arch_keyword = match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => bail!("暂不支持的架构: {arch}"),
    };

    let target = format!("clash-linux-{arch_keyword}.tar.gz");
    for asset in assets {
        if asset.name.to_lowercase() == target {
            return Ok(asset.clone());
        }
    }

    // 模糊匹配回退
    for asset in assets {
        let name = asset.name.to_lowercase();
        if name.contains("linux") && name.contains(arch_keyword) && name.ends_with(".tar.gz") {
            return Ok(asset.clone());
        }
    }

    bail!("未找到匹配的 CLI 发布资产 (arch={arch_keyword})")
}

fn find_extracted_binary(dir: &Path) -> Result<std::path::PathBuf> {
    let entries = fs::read_dir(dir).context("读取解压目录失败")?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("clash") && !name_str.ends_with(".tar.gz") {
            let path = entry.path();
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    bail!("解压后未找到 clash 二进制文件")
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
