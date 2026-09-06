use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use zip::ZipArchive;

use crate::cli::{UiCommand, UiInstallArgs};
use crate::constants;
use crate::http::{build_http_client, download_candidates, download_to_file};
use crate::machine::{ErrorCode, coded_error};
use crate::output::{is_machine_mode, print_machine};
use crate::paths::app_paths;

pub fn run(command: Option<UiCommand>) -> Result<()> {
    match command.unwrap_or(UiCommand::Status) {
        UiCommand::Install(args) => cmd_install(args),
        UiCommand::Status => cmd_status(),
        UiCommand::Url => cmd_url(),
        UiCommand::Open => cmd_open(),
    }
}

pub fn resolve_ui_dir(workdir: Option<&Path>) -> Result<PathBuf> {
    let workdir = match workdir {
        Some(p) => p.to_path_buf(),
        None => app_paths()?.runtime_dir,
    };
    Ok(workdir.join(constants::DEFAULT_EXTERNAL_UI))
}

pub fn ui_is_installed(ui_dir: &Path) -> bool {
    ui_dir.join("index.html").is_file()
}

pub fn dashboard_url_from_runtime() -> Result<String> {
    let paths = app_paths()?;
    let controller = if is_machine_mode() {
        read_controller_strict(&paths.runtime_config_file)?
    } else {
        read_controller(&paths.runtime_config_file)
            .unwrap_or_else(|| constants::DEFAULT_CONTROLLER.to_string())
    };
    Ok(format!(
        "{}/ui",
        normalize_controller_url(&controller).trim_end_matches('/')
    ))
}

fn read_controller_strict(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(coded_error(
            ErrorCode::RuntimeConfigRequired,
            format!(
                "需要已渲染 runtime 配置才能确定 Dashboard URL: {}",
                path.display()
            ),
        ));
    }
    let content = fs::read_to_string(path)
        .with_context(|| format!("读取 runtime 配置失败: {}", path.display()))?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content).map_err(|err| {
        coded_error(
            ErrorCode::ConfigInvalid,
            format!("解析 runtime 配置失败 {}: {err}", path.display()),
        )
    })?;
    root.as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String("external-controller".into())))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            coded_error(
                ErrorCode::ConfigInvalid,
                "runtime 缺少 external-controller，拒绝猜测 Dashboard 地址",
            )
        })
}

/// 解压 metacubexd / GitHub zip：剥掉单一顶层目录，拒绝 `..` 路径。
pub fn extract_web_ui_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file =
        File::open(archive).with_context(|| format!("打开 zip 失败: {}", archive.display()))?;
    let mut zip = ZipArchive::new(file).context("解析 zip 失败")?;
    let strip = common_root_prefix(&mut zip)?;

    if dest.exists() {
        fs::remove_dir_all(dest)
            .with_context(|| format!("清理旧 UI 目录失败: {}", dest.display()))?;
    }
    fs::create_dir_all(dest).with_context(|| format!("创建 UI 目录失败: {}", dest.display()))?;

    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .with_context(|| format!("读取 zip 条目 {i} 失败"))?;
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let rel = if let Some(prefix) = &strip {
            match enclosed.strip_prefix(prefix) {
                Ok(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
                _ => continue,
            }
        } else {
            enclosed.to_path_buf()
        };
        if rel
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            bail!("zip 含非法路径: {}", rel.display());
        }
        let out_path = dest.join(&rel);
        if entry.is_dir() {
            fs::create_dir_all(&out_path)
                .with_context(|| format!("创建目录失败: {}", out_path.display()))?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("创建目录失败: {}", parent.display()))?;
        }
        let mut outfile =
            File::create(&out_path).with_context(|| format!("写入失败: {}", out_path.display()))?;
        io::copy(&mut entry, &mut outfile)
            .with_context(|| format!("解压失败: {}", out_path.display()))?;
    }

    if !dest.join("index.html").is_file() {
        bail!(
            "解压后未找到 index.html（目录: {}）。请换官方 metacubexd gh-pages zip。",
            dest.display()
        );
    }
    Ok(())
}

fn common_root_prefix(zip: &mut ZipArchive<File>) -> Result<Option<PathBuf>> {
    let mut prefix: Option<PathBuf> = None;
    for i in 0..zip.len() {
        let entry = zip.by_index(i).context("扫描 zip 失败")?;
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        if name.as_os_str().is_empty() {
            continue;
        }
        let first = name
            .components()
            .next()
            .map(|c| PathBuf::from(c.as_os_str()));
        match (&mut prefix, first) {
            (None, Some(p)) => prefix = Some(p),
            (Some(cur), Some(p)) if *cur != p => return Ok(None),
            _ => {}
        }
    }
    // 只有「全部在同一顶层目录下」才剥离，根目录已有文件则不剥。
    if let Some(root) = &prefix {
        let mut has_nested = false;
        for i in 0..zip.len() {
            let entry = zip.by_index(i).context("扫描 zip 失败")?;
            let Some(name) = entry.enclosed_name() else {
                continue;
            };
            if name.components().count() > 1 {
                has_nested = true;
                break;
            }
        }
        if has_nested {
            return Ok(Some(root.clone()));
        }
    }
    Ok(None)
}

fn cmd_install(args: UiInstallArgs) -> Result<()> {
    let ui_dir = resolve_ui_dir(args.workdir.as_deref())?;
    if ui_is_installed(&ui_dir) && !args.force {
        if is_machine_mode() {
            return print_machine(&serde_json::json!({
                "reused": true,
                "path": ui_dir.display().to_string(),
            }));
        }
        println!("Web UI 已存在: {}", ui_dir.display());
        println!("覆盖安装请加 --force。");
        println!(
            "打开: {}",
            dashboard_url_from_runtime().unwrap_or_else(|_| "http://127.0.0.1:9090/ui".into())
        );
        return Ok(());
    }

    let client = build_http_client()?;
    let candidates = download_candidates(constants::METACUBEXD_GITHUB_ZIP, args.mirror);
    if let Some(parent) = ui_dir.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }
    let tmp_zip = ui_dir
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("metacubexd-{}.zip", std::process::id()));
    let mut last_err = None;
    let mut source = None;
    for url in &candidates {
        match download_to_file(&client, url, &tmp_zip) {
            Ok(()) => {
                source = Some(url.clone());
                break;
            }
            Err(err) => last_err = Some(err),
        }
    }
    let source = match source {
        Some(v) => v,
        None => {
            let _ = fs::remove_file(&tmp_zip);
            bail!(
                "下载 metacubexd 失败: {}",
                last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "无可用镜像".into())
            );
        }
    };

    let extract_result = extract_web_ui_zip(&tmp_zip, &ui_dir);
    let _ = fs::remove_file(&tmp_zip);
    extract_result?;

    if is_machine_mode() {
        let url = dashboard_url_from_runtime().ok();
        return print_machine(&serde_json::json!({
            "reused": false,
            "path": ui_dir.display().to_string(),
            "source": source,
            "url": url,
            "url_available": url.is_some(),
        }));
    }
    let url = dashboard_url_from_runtime().unwrap_or_else(|_| "http://127.0.0.1:9090/ui".into());
    println!("已安装 Web UI (metacubexd): {}", ui_dir.display());
    println!("来源: {}", source);
    println!("浏览器打开: {}", url);
    println!("需内核在跑: clash service start");
    Ok(())
}

fn cmd_status() -> Result<()> {
    let ui_dir = resolve_ui_dir(None)?;
    let installed = ui_is_installed(&ui_dir);
    if is_machine_mode() {
        let url = dashboard_url_from_runtime().ok();
        return print_machine(&serde_json::json!({
            "installed": installed,
            "path": ui_dir.display().to_string(),
            "url": url,
            "url_available": url.is_some(),
            "name": constants::DEFAULT_EXTERNAL_UI_NAME,
        }));
    }
    let url = dashboard_url_from_runtime().unwrap_or_else(|_| "http://127.0.0.1:9090/ui".into());
    if installed {
        println!("Web UI: 已安装 ({})", constants::DEFAULT_EXTERNAL_UI_NAME);
        println!("目录: {}", ui_dir.display());
        println!("地址: {}", url);
        println!("打开: clash ui open");
    } else {
        println!("Web UI: 未安装");
        println!("将安装到: {}", ui_dir.display());
        println!("执行: clash ui install");
    }
    Ok(())
}

fn cmd_url() -> Result<()> {
    let url = dashboard_url_from_runtime()?;
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "url": url,
        }));
    }
    println!("{}", url);
    Ok(())
}

fn cmd_open() -> Result<()> {
    let ui_dir = resolve_ui_dir(None)?;
    let url = dashboard_url_from_runtime()?;
    if !ui_is_installed(&ui_dir) {
        bail!("尚未安装 Web UI，请先执行: clash ui install");
    }
    let opened = try_open_browser(&url);
    if is_machine_mode() {
        return print_machine(&serde_json::json!({
            "url": url,
            "opened": opened,
        }));
    }
    if opened {
        println!("已尝试打开: {}", url);
    } else {
        println!("请在浏览器打开: {}", url);
    }
    Ok(())
}

fn try_open_browser(url: &str) -> bool {
    let bin = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(bin)
        .arg(url)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn read_controller(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let root: serde_yaml::Value = serde_yaml::from_str(&content).ok()?;
    root.as_mapping()
        .and_then(|m| m.get(serde_yaml::Value::String("external-controller".into())))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn normalize_controller_url(value: &str) -> String {
    if value.starts_with("http://") || value.starts_with("https://") {
        value.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", value.trim_end_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};
    use zip::ZipWriter;
    use zip::write::FileOptions;

    fn temp_dir(prefix: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|v| v.as_nanos())
            .unwrap_or(0);
        path.push(format!(
            "clash_ui_{prefix}_{}_{}",
            std::process::id(),
            nanos
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let file = File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        let opts = FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extract_strips_github_root_folder() {
        let dir = temp_dir("zip");
        let zip_path = dir.join("ui.zip");
        write_zip(
            &zip_path,
            &[
                ("metacubexd-gh-pages/index.html", "<html>ok</html>"),
                ("metacubexd-gh-pages/assets/app.js", "console.log(1)"),
            ],
        );
        let dest = dir.join("ui");
        extract_web_ui_zip(&zip_path, &dest).expect("解压失败");
        assert!(dest.join("index.html").is_file());
        assert!(dest.join("assets").join("app.js").is_file());
        assert!(!dest.join("metacubexd-gh-pages").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_rejects_parent_dir_entries() {
        let dir = temp_dir("slip");
        let zip_path = dir.join("bad.zip");
        write_zip(&zip_path, &[("../evil.html", "x")]);
        let dest = dir.join("ui");
        let err = extract_web_ui_zip(&zip_path, &dest).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("index.html") || msg.contains("非法"),
            "应拒绝或不落下 index: {msg}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ui_is_installed_requires_index_html() {
        let dir = temp_dir("idx");
        assert!(!ui_is_installed(&dir));
        fs::write(dir.join("index.html"), "<html>").unwrap();
        assert!(ui_is_installed(&dir));
        let _ = fs::remove_dir_all(&dir);
    }
}
