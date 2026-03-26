/// 集中管理的默认常量，避免在各模块中重复定义

// --- 端口 ---
pub const DEFAULT_MIXED_PORT: u16 = 7890;
pub const DEFAULT_SOCKS_PORT: u16 = 7891;
pub const DEFAULT_REDIR_PORT: u16 = 7892;

// --- 地址 ---
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1";
pub const DEFAULT_CONTROLLER: &str = "127.0.0.1:9090";

// --- 服务 ---
pub const DEFAULT_SERVICE_NAME: &str = "clash-mihomo";
pub const DEFAULT_SYSTEM_SERVICE_UNIT: &str = "clash-mihomo.service";

// --- 代理 ---
pub const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1";

// --- Dashboard / UI ---
pub const DEFAULT_EXTERNAL_UI: &str = "ui";
pub const DEFAULT_EXTERNAL_UI_NAME: &str = "metacubexd";
pub const DEFAULT_EXTERNAL_UI_URL: &str =
    "https://ghfast.top/https://github.com/MetaCubeX/metacubexd/archive/refs/heads/gh-pages.zip";
