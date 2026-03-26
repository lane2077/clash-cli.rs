use std::fs;
use std::process::Command;

pub(super) fn detect_bridge_interfaces() -> Vec<String> {
    let output = match Command::new("ip")
        .args(["-o", "link", "show", "type", "bridge"])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut ifaces: Vec<String> = stdout
        .lines()
        .filter_map(|line| {
            // 格式: "3: docker0: <...> ..."
            let after_num = line.split_once(':')?;
            let name_part = after_num.1.trim();
            let name = name_part.split_once(':')?.0.trim();
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect();
    ifaces.sort();
    ifaces.dedup();
    ifaces
}

/// 动态检测 host 网络模式的容器进程 UID（无硬编码）
///
/// 原理：遍历 /proc，找到满足以下条件的进程：
/// 1. 网络命名空间与宿主机一致（host 网络模式）
/// 2. cgroup 属于容器（docker / containerd / podman）
/// 3. UID 非 0（root 不需要排除）
pub(super) fn detect_exclude_uids() -> Vec<u32> {
    let host_netns = match fs::read_link("/proc/1/ns/net") {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let proc_dir = match fs::read_dir("/proc") {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut uids = Vec::new();
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if !name_str.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let pid_path = entry.path();
        // 检查网络命名空间是否与宿主机一致
        let proc_netns = match fs::read_link(pid_path.join("ns/net")) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if proc_netns != host_netns {
            continue;
        }
        // 检查是否在容器中（cgroup 包含容器标识）
        let cgroup = match fs::read_to_string(pid_path.join("cgroup")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let in_container = cgroup.contains("docker")
            || cgroup.contains("containerd")
            || cgroup.contains("podman")
            || cgroup.contains("/lxc/");
        if !in_container {
            continue;
        }
        // 读取 UID
        let status = match fs::read_to_string(pid_path.join("status")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                if let Some(uid_str) = rest.trim().split_whitespace().next() {
                    if let Ok(uid) = uid_str.parse::<u32>() {
                        if uid != 0 {
                            uids.push(uid);
                        }
                    }
                }
                break;
            }
        }
    }
    uids.sort();
    uids.dedup();
    uids
}
