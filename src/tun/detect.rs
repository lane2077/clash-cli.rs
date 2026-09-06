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
