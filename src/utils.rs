use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

/// 只检查可执行文件是否存在于 PATH，不通过猜测 `--version` 参数来探测。
/// 某些系统工具（例如 macOS networksetup）并不支持通用版本参数。
pub fn command_exists(binary: &str) -> bool {
    let candidate = Path::new(binary);
    if candidate.components().count() > 1 {
        return is_executable(candidate);
    }
    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| is_executable(&dir.join(binary))))
        .unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// 执行命令并检查是否成功，抑制所有输出。
pub(crate) fn check_cmd_success(binary: &str, args: &[&str]) -> bool {
    Command::new(binary)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub(crate) fn is_root_user() -> bool {
    let output = Command::new("id").arg("-u").output();
    match output {
        Ok(v) if v.status.success() => String::from_utf8_lossy(&v.stdout).trim() == "0",
        _ => false,
    }
}

pub(crate) fn is_macos() -> bool {
    env::consts::OS == "macos"
}

pub(crate) fn is_linux() -> bool {
    env::consts::OS == "linux"
}

pub(crate) fn ensure_supported_host() -> Result<()> {
    if !matches!(env::consts::OS, "linux" | "macos") {
        bail!("当前仅支持 Linux 与 macOS");
    }
    Ok(())
}

pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .unwrap_or(0)
}

pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建目录失败: {}", parent.display()))?;
    }

    let mut tmp_path = path.to_path_buf();
    let pid = std::process::id();
    let ts = now_unix();
    let suffix = format!(".clash_cli_tmp_{pid}_{ts}");
    let filename = path
        .file_name()
        .map(|v| format!("{}{}", v.to_string_lossy(), suffix))
        .context("无效路径：无法获取文件名")?;
    tmp_path.set_file_name(filename);

    let mode = fs::metadata(path)
        .map(|meta| meta.permissions().mode() & 0o777)
        .unwrap_or(0o600);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(mode)
        .open(&tmp_path)
        .with_context(|| format!("创建临时文件失败: {}", tmp_path.display()))?;
    fs::set_permissions(&tmp_path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("设置临时文件权限失败: {}", tmp_path.display()))?;
    file.write_all(data)
        .with_context(|| format!("写入临时文件失败: {}", tmp_path.display()))?;
    file.sync_all()
        .with_context(|| format!("刷新临时文件失败: {}", tmp_path.display()))?;

    fs::rename(&tmp_path, path).with_context(|| {
        format!(
            "原子替换文件失败: {} -> {}",
            tmp_path.display(),
            path.display()
        )
    })?;

    if let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(())
}

pub(crate) fn write_atomic_text(path: &Path, text: &str) -> Result<()> {
    write_atomic(path, text.as_bytes())
}

pub(crate) fn normalize_unit_name(name: &str) -> String {
    if name.ends_with(".service") {
        name.to_string()
    } else {
        format!("{name}.service")
    }
}
