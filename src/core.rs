use std::env;
use std::fs;
use std::fs::File;
use std::io::{self, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::Client;
use serde::Deserialize;

use crate::cli::{Amd64Variant, CoreCommand, CoreInstallArgs, CoreUpgradeArgs, MirrorSource};
use crate::output::{is_json_mode, print_json};
use crate::paths::app_paths;

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
struct GitHubAsset {
    name: String,
    browser_download_url: String,
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
    ensure_linux_host()?;
    let request = CoreInstallRequest {
        version: args.version,
        mirror: args.mirror,
        amd64_variant: args.amd64_variant,
        force: args.force,
    };
    install_mihomo_core(request)
}

fn cmd_upgrade(args: CoreUpgradeArgs) -> Result<()> {
    ensure_linux_host()?;
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

    decompress_gzip_to_file(&temp_gz_path, &temp_bin_path)?;
    set_executable(&temp_bin_path)?;

    if installed_binary.exists() {
        fs::remove_file(&installed_binary).context("替换旧内核失败")?;
    }
    fs::rename(&temp_bin_path, &installed_binary).context("落盘新内核失败")?;

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

fn ensure_linux_host() -> Result<()> {
    if env::consts::OS != "linux" {
        bail!("当前仅支持 Linux 平台");
    }
    Ok(())
}

fn build_http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(20))
        .user_agent("clash-cli/0.1")
        .build()
        .context("创建 HTTP 客户端失败")
}

fn fetch_release(client: &Client, version: &str) -> Result<GitHubRelease> {
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

fn select_release_asset(
    assets: &[GitHubAsset],
    amd64_variant: Amd64Variant,
) -> Result<GitHubAsset> {
    let arch = env::consts::ARCH;
    let mut linux_assets: Vec<GitHubAsset> = assets
        .iter()
        .filter(|asset| {
            let name = asset.name.to_lowercase();
            name.contains("linux") && name.ends_with(".gz")
        })
        .cloned()
        .collect();

    if linux_assets.is_empty() {
        bail!("{GITHUB_REPO} 当前版本未找到 Linux 资产");
    }

    linux_assets.sort_by(|a, b| a.name.cmp(&b.name));

    // 资产匹配单独抽离，后续可替换成可配置规则。
    match arch {
        "x86_64" => pick_amd64_asset(&linux_assets, amd64_variant),
        "aarch64" => pick_asset_by_keywords(&linux_assets, &["arm64", "aarch64"]),
        "arm" => pick_asset_by_keywords(&linux_assets, &["armv7", "armv6", "arm"]),
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

fn download_candidates(original_url: &str, mirror: MirrorSource) -> Vec<String> {
    let mut urls = Vec::new();
    let ghfast_url = format!("https://ghfast.top/{original_url}");

    match mirror {
        MirrorSource::Auto => {
            if original_url.starts_with("https://github.com/") {
                urls.push(ghfast_url);
            }
            urls.push(original_url.to_string());
        }
        MirrorSource::Ghfast => urls.push(ghfast_url),
        MirrorSource::Github => urls.push(original_url.to_string()),
    }

    urls
}

fn download_to_file(client: &Client, url: &str, output_path: &Path) -> Result<()> {
    let mut response = client
        .get(url)
        .send()
        .with_context(|| format!("下载请求失败: {url}"))?
        .error_for_status()
        .with_context(|| format!("下载响应失败: {url}"))?;

    let mut file = File::create(output_path)
        .with_context(|| format!("创建下载文件失败: {}", output_path.display()))?;
    io::copy(&mut response, &mut file)
        .with_context(|| format!("写入文件失败: {}", output_path.display()))?;
    file.flush()
        .with_context(|| format!("刷新文件失败: {}", output_path.display()))?;
    Ok(())
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
    fs::write(path, content).with_context(|| format!("写入元信息失败: {}", path.display()))
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
