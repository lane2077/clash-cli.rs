use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

const DEFAULT_NO_PROXY: &str = "localhost,127.0.0.1,::1";
const DEFAULT_SERVICE_NAME: &str = "clash-mihomo";
const DEFAULT_PROFILE_NAME: &str = "main";

#[derive(Parser)]
#[command(name = "clash", about = "面向 Linux 的 Clash 命令行工具")]
pub struct Cli {
    #[arg(long, global = true, help = "以 JSON 格式输出")]
    pub json: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "管理终端代理（start/stop/env/auto/status）")]
    Proxy {
        #[command(subcommand)]
        command: ProxyCommand,
    },
    #[command(about = "管理 mihomo 内核安装、升级、版本与路径")]
    Core {
        #[command(subcommand)]
        command: CoreCommand,
    },
    #[command(about = "管理 systemd 服务（install/start/stop/status/log）")]
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
    },
    #[command(about = "管理 TUN 模式（诊断/开启/关闭/状态）")]
    Tun {
        #[command(subcommand)]
        command: TunCommand,
    },
    #[command(about = "管理订阅 profile（add/fetch/render/validate）")]
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    #[command(about = "访问 mihomo external-controller API")]
    Api {
        #[command(subcommand)]
        command: ApiCommand,
    },
    #[command(about = "一键初始化/收敛部署流程")]
    Setup {
        #[command(subcommand)]
        command: SetupCommand,
    },
}

#[derive(Subcommand)]
pub enum ProxyCommand {
    #[command(about = "写入代理状态（默认端口来自 runtime 配置）")]
    Start(StartArgs),
    #[command(about = "清理代理状态，可选移除 shell 自动启用钩子")]
    Stop(StopArgs),
    #[command(about = "查看当前代理状态与自动启用状态")]
    Status,
    #[command(about = "输出当前终端可执行的环境变量脚本（on/off）")]
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },
    #[command(about = "管理新终端自动启用代理（on/off/status）")]
    Auto {
        #[command(subcommand)]
        action: AutoAction,
    },
}

#[derive(Subcommand)]
pub enum CoreCommand {
    #[command(about = "安装指定版本 mihomo 内核")]
    Install(CoreInstallArgs),
    #[command(about = "升级 mihomo 到最新可用版本")]
    Upgrade(CoreUpgradeArgs),
    #[command(about = "输出当前已安装内核版本")]
    Version,
    #[command(about = "输出当前生效内核二进制路径")]
    Path,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    #[command(about = "安装 systemd service unit 并按需启用/启动")]
    Install(ServiceInstallArgs),
    #[command(about = "卸载 systemd service unit，可选清理运行目录")]
    Uninstall(ServiceUninstallArgs),
    #[command(about = "启用开机自启（systemctl enable）")]
    Enable(ServiceTargetArgs),
    #[command(about = "关闭开机自启（systemctl disable）")]
    Disable(ServiceTargetArgs),
    #[command(about = "启动服务（systemctl start）")]
    Start(ServiceTargetArgs),
    #[command(about = "停止服务（systemctl stop）")]
    Stop(ServiceTargetArgs),
    #[command(about = "重启服务（systemctl restart）")]
    Restart(ServiceTargetArgs),
    #[command(about = "查看服务状态（systemctl status）")]
    Status(ServiceTargetArgs),
    #[command(about = "查看服务日志（journalctl）")]
    Log(ServiceLogArgs),
}

#[derive(Subcommand)]
pub enum TunCommand {
    #[command(about = "诊断 tun 运行前置条件（能力/内核/配置）")]
    Doctor,
    #[command(about = "开启 tun 配置并按需下发数据面规则")]
    On(TunApplyArgs),
    #[command(about = "关闭 tun 配置并清理数据面规则")]
    Off(TunApplyArgs),
    #[command(about = "查看 tun 配置、规则和服务实际状态")]
    Status(TunStatusArgs),
}

#[derive(Subcommand)]
pub enum ProfileCommand {
    #[command(about = "添加订阅 profile")]
    Add(ProfileAddArgs),
    #[command(about = "列出所有 profile 与当前 active")]
    List,
    #[command(about = "切换当前 active profile")]
    Use(ProfileUseArgs),
    #[command(about = "拉取指定 profile 的最新订阅内容")]
    Fetch(ProfileFetchArgs),
    #[command(about = "删除 profile")]
    Remove(ProfileRemoveArgs),
    #[command(about = "将 profile 渲染到运行配置 runtime/config.yaml")]
    Render(ProfileRenderArgs),
    #[command(about = "校验 profile YAML 基础合法性")]
    Validate(ProfileValidateArgs),
}

#[derive(Subcommand)]
pub enum ApiCommand {
    #[command(about = "查看 external-controller 连接状态")]
    Status(ApiCommonArgs),
    #[command(about = "读取或设置运行模式（rule/global/direct/script）")]
    Mode {
        #[command(subcommand)]
        action: ApiModeCommand,
        #[command(flatten)]
        common: ApiCommonArgs,
    },
    #[command(about = "查看代理组与节点摘要")]
    Proxies(ApiCommonArgs),
    #[command(about = "查看当前连接摘要")]
    Connections(ApiCommonArgs),
    #[command(about = "输出 Dashboard 访问地址（含 controller/ui 元信息）")]
    UiUrl(ApiCommonArgs),
}

#[derive(Subcommand)]
pub enum ApiModeCommand {
    #[command(about = "读取当前模式")]
    Get,
    #[command(about = "设置当前模式")]
    Set(ApiModeSetArgs),
}

#[derive(Subcommand)]
pub enum SetupCommand {
    #[command(about = "一键初始化（内核 + 订阅 + 渲染 + service + tun）")]
    Init(SetupInitArgs),
    #[command(about = "收敛历史配置到系统目录（/etc/clash-cli）并可选应用")]
    Unify(SetupUnifyArgs),
}

#[derive(Args, Clone)]
pub struct ProfileAddArgs {
    #[arg(long, help = "profile 名称")]
    pub name: String,
    #[arg(long, help = "订阅 URL")]
    pub url: String,
    #[arg(long, help = "添加后设为当前 profile")]
    pub use_profile: bool,
    #[arg(long, help = "添加时不立即拉取")]
    pub no_fetch: bool,
}

#[derive(Args, Clone)]
pub struct ProfileUseArgs {
    #[arg(long, help = "profile 名称")]
    pub name: String,
    #[arg(long, help = "切换后立即渲染到 runtime/config.yaml")]
    pub apply: bool,
    #[arg(long, help = "切换后强制拉取最新订阅（隐含 --apply）")]
    pub fetch: bool,
    #[arg(
        long,
        default_value = DEFAULT_SERVICE_NAME,
        help = "apply 后联动重启的 systemd 服务名"
    )]
    pub service_name: String,
    #[arg(long, help = "apply 后仅渲染，不自动重启服务")]
    pub no_restart: bool,
}

#[derive(Args, Clone)]
pub struct ProfileFetchArgs {
    #[arg(long, help = "profile 名称")]
    pub name: String,
    #[arg(long, help = "忽略缓存强制更新")]
    pub force: bool,
}

#[derive(Args, Clone)]
pub struct ProfileRemoveArgs {
    #[arg(long, help = "profile 名称")]
    pub name: String,
}

#[derive(Args, Clone)]
pub struct ProfileRenderArgs {
    #[arg(long, help = "profile 名称，默认使用当前 active")]
    pub name: Option<String>,
    #[arg(long, help = "输出配置路径，默认 runtime/config.yaml")]
    pub output: Option<PathBuf>,
    #[arg(long, help = "渲染时忽略 mixin.yaml")]
    pub no_mixin: bool,
    #[arg(long, help = "渲染时跟随订阅中的监听端口与控制器设置")]
    pub follow_subscription_port: bool,
}

#[derive(Args, Clone)]
pub struct ProfileValidateArgs {
    #[arg(long, help = "profile 名称，默认使用当前 active")]
    pub name: Option<String>,
}

#[derive(Args, Clone)]
pub struct ApiCommonArgs {
    #[arg(long, help = "external-controller 地址，例如 127.0.0.1:9090")]
    pub controller: Option<String>,
    #[arg(long, help = "external-controller secret")]
    pub secret: Option<String>,
    #[arg(long, default_value_t = 15, help = "API 请求超时秒数")]
    pub timeout_secs: u64,
}

#[derive(Args, Clone)]
pub struct ApiModeSetArgs {
    #[arg(value_enum, help = "目标模式")]
    pub mode: ApiModeValue,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ApiModeValue {
    Rule,
    Global,
    Direct,
    Script,
}

impl ApiModeValue {
    pub fn as_api_str(self) -> &'static str {
        match self {
            ApiModeValue::Rule => "rule",
            ApiModeValue::Global => "global",
            ApiModeValue::Direct => "direct",
            ApiModeValue::Script => "script",
        }
    }
}

#[derive(Args, Clone)]
pub struct TunApplyArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "联动重启的 systemd 服务名")]
    pub name: String,
    #[arg(long, help = "联动操作 user 级服务（systemctl --user）")]
    pub user: bool,
    #[arg(long, help = "仅修改配置，不自动重启服务")]
    pub no_restart: bool,
}

#[derive(Args, Clone)]
pub struct TunStatusArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "用于检查状态的 systemd 服务名")]
    pub name: String,
    #[arg(long, help = "检查 user 级服务（systemctl --user）")]
    pub user: bool,
}

#[derive(Args)]
pub struct StartArgs {
    #[arg(
        long,
        help = "代理监听地址（默认读取 runtime bind-address，回退 127.0.0.1）"
    )]
    pub host: Option<String>,
    #[arg(
        long,
        help = "HTTP/HTTPS 代理端口（默认读取 runtime mixed-port/port，回退 7890）"
    )]
    pub http_port: Option<u16>,
    #[arg(
        long,
        help = "SOCKS5 代理端口（默认读取 runtime socks-port/mixed-port，回退 7891）"
    )]
    pub socks_port: Option<u16>,
    #[arg(long, default_value = DEFAULT_NO_PROXY, help = "直连名单 no_proxy")]
    pub no_proxy: String,
    #[arg(long, help = "为新终端自动启用代理")]
    pub auto: bool,
    #[arg(long, value_enum, requires = "auto", help = "指定 shell 类型")]
    pub shell: Option<ShellKind>,
    #[arg(long, help = "仅输出 export 脚本")]
    pub print_env: bool,
}

#[derive(Args)]
pub struct StopArgs {
    #[arg(long, help = "同时移除自动启用钩子")]
    pub auto_off: bool,
    #[arg(long, value_enum, requires = "auto_off", help = "指定 shell 类型")]
    pub shell: Option<ShellKind>,
    #[arg(long, help = "仅输出 unset 脚本")]
    pub print_env: bool,
}

#[derive(Args)]
pub struct CoreInstallArgs {
    #[arg(
        long,
        default_value = "latest",
        help = "内核版本，如 latest 或 v1.19.20"
    )]
    pub version: String,
    #[arg(
        long,
        value_enum,
        default_value_t = MirrorSource::Auto,
        help = "下载镜像策略"
    )]
    pub mirror: MirrorSource,
    #[arg(
        long,
        value_enum,
        default_value_t = Amd64Variant::Auto,
        help = "x86_64 资产偏好"
    )]
    pub amd64_variant: Amd64Variant,
    #[arg(long, help = "已安装也强制重装")]
    pub force: bool,
}

#[derive(Args)]
pub struct CoreUpgradeArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = MirrorSource::Auto,
        help = "下载镜像策略"
    )]
    pub mirror: MirrorSource,
    #[arg(
        long,
        value_enum,
        default_value_t = Amd64Variant::Auto,
        help = "x86_64 资产偏好"
    )]
    pub amd64_variant: Amd64Variant,
    #[arg(long, help = "强制重装")]
    pub force: bool,
}

#[derive(Args, Clone)]
pub struct SetupInitArgs {
    #[arg(long, help = "订阅 URL")]
    pub profile_url: String,
    #[arg(long, default_value = DEFAULT_PROFILE_NAME, help = "profile 名称")]
    pub profile_name: String,
    #[arg(
        long,
        default_value = "latest",
        help = "内核版本，如 latest 或 v1.19.20"
    )]
    pub core_version: String,
    #[arg(
        long,
        value_enum,
        default_value_t = MirrorSource::Auto,
        help = "下载镜像策略"
    )]
    pub mirror: MirrorSource,
    #[arg(
        long,
        value_enum,
        default_value_t = Amd64Variant::Auto,
        help = "x86_64 资产偏好"
    )]
    pub amd64_variant: Amd64Variant,
    #[arg(long, help = "覆盖已安装内核")]
    pub force_core: bool,
    #[arg(
        long,
        default_value = "/usr/local/bin/mihomo",
        help = "mihomo 安装路径"
    )]
    pub binary: PathBuf,
    #[arg(long, default_value = "/var/lib/clash-cli", help = "service 工作目录")]
    pub workdir: PathBuf,
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "systemd 服务名")]
    pub service_name: String,
    #[arg(long, help = "初始化完成后不自动开启 tun")]
    pub no_tun: bool,
}

#[derive(Args, Clone)]
pub struct SetupUnifyArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "联动重启的 systemd 服务名")]
    pub service_name: String,
    #[arg(long, help = "仅收敛 profile，不渲染与重启服务")]
    pub no_apply: bool,
    #[arg(long, help = "仅合并 profile，不替换历史目录为 /etc/clash-cli 软链接")]
    pub no_link: bool,
}

#[derive(Args, Clone)]
pub struct ServiceTargetArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "systemd 服务名")]
    pub name: String,
    #[arg(long, help = "操作 user 级服务（systemctl --user）")]
    pub user: bool,
}

#[derive(Args)]
pub struct ServiceInstallArgs {
    #[command(flatten)]
    pub target: ServiceTargetArgs,
    #[arg(long, help = "指定 mihomo 二进制路径")]
    pub binary: Option<PathBuf>,
    #[arg(long, help = "指定 mihomo 配置文件路径")]
    pub config: Option<PathBuf>,
    #[arg(long, help = "指定工作目录")]
    pub workdir: Option<PathBuf>,
    #[arg(long, help = "覆盖已存在的 unit 文件")]
    pub force: bool,
    #[arg(long, help = "安装后不自动 enable")]
    pub no_enable: bool,
    #[arg(long, help = "安装后不自动 start")]
    pub no_start: bool,
}

#[derive(Args)]
pub struct ServiceUninstallArgs {
    #[command(flatten)]
    pub target: ServiceTargetArgs,
    #[arg(long, help = "同时清理 runtime 目录（包含配置）")]
    pub purge: bool,
}

#[derive(Args)]
pub struct ServiceLogArgs {
    #[command(flatten)]
    pub target: ServiceTargetArgs,
    #[arg(short = 'f', long, help = "持续跟随日志")]
    pub follow: bool,
    #[arg(short = 'n', long, default_value_t = 100, help = "读取最近 N 行")]
    pub lines: usize,
}

#[derive(Subcommand)]
pub enum EnvAction {
    #[command(about = "输出 export 代理变量脚本")]
    On,
    #[command(about = "输出 unset 代理变量脚本")]
    Off,
}

#[derive(Subcommand)]
pub enum AutoAction {
    #[command(about = "在 shell 启动文件中写入自动启用代理钩子")]
    On {
        #[arg(long, value_enum, help = "指定 shell 类型")]
        shell: Option<ShellKind>,
    },
    #[command(about = "从 shell 启动文件移除自动启用代理钩子")]
    Off {
        #[arg(long, value_enum, help = "指定 shell 类型")]
        shell: Option<ShellKind>,
    },
    #[command(about = "查看自动启用代理钩子状态")]
    Status {
        #[arg(long, value_enum, help = "指定 shell 类型")]
        shell: Option<ShellKind>,
    },
}

impl AutoAction {
    pub fn shell(&self) -> Option<ShellKind> {
        match self {
            AutoAction::On { shell } => *shell,
            AutoAction::Off { shell } => *shell,
            AutoAction::Status { shell } => *shell,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ShellKind {
    Bash,
    Zsh,
}

impl ShellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum MirrorSource {
    Auto,
    Ghfast,
    Github,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Amd64Variant {
    Auto,
    Compatible,
    V3,
}
