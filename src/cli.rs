use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::constants;

const DEFAULT_NO_PROXY: &str = constants::DEFAULT_NO_PROXY;
const DEFAULT_SERVICE_NAME: &str = constants::DEFAULT_SERVICE_NAME;
const DEFAULT_SUB_NAME: &str = "main";

#[derive(Parser)]
#[command(
    name = "clash",
    version,
    about = "面向 Linux 与 macOS 的 Clash 命令行工具"
)]
pub struct Cli {
    #[arg(
        long,
        global = true,
        help = "启用稳定 Machine Contract v0；显式、非交互、与 TTY 无关"
    )]
    pub machine: bool,
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    #[command(about = "查看 Machine Contract v0 能力与约束")]
    Contract,
    #[command(about = "仪表板（metacubexd Web UI）")]
    Ui {
        #[command(subcommand)]
        command: Option<UiCommand>,
    },
    #[command(about = "出站模式（规则/全局/直连）", subcommand_required = false)]
    Mode {
        #[command(subcommand)]
        action: Option<ApiModeCommand>,
        #[command(flatten)]
        common: ApiCommonArgs,
    },
    #[command(name = "sub", about = "订阅（list/use/update/add；mixin 覆盖）")]
    Sub {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    #[command(
        about = "代理（查看代理组并切换当前节点）",
        subcommand_required = false
    )]
    Proxy {
        #[command(subcommand)]
        command: Option<ProxyCommand>,
    },
    #[command(about = "系统代理（桌面 HTTP/SOCKS，浏览器等）")]
    System {
        #[command(subcommand)]
        action: SystemProxyAction,
    },
    #[command(about = "TUN 模式（诊断/开启/关闭/状态）")]
    Tun {
        #[command(subcommand)]
        command: TunCommand,
    },
    #[command(about = "复制环境变量（供 eval \"$(clash env on)\"）")]
    Env {
        #[command(subcommand)]
        action: EnvAction,
    },
    #[command(about = "管理 mihomo 内核安装、升级、版本与路径")]
    Core {
        #[command(subcommand)]
        command: CoreCommand,
    },
    #[command(about = "管理 systemd / launchd 服务（install/start/stop/status/log）")]
    Service {
        #[command(subcommand)]
        command: ServiceCommand,
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
    #[command(about = "更新 clash CLI 自身到最新版本")]
    Update {
        #[command(subcommand)]
        command: UpdateCommand,
    },
}

impl Commands {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Contract => "contract.describe",
            Self::Ui { command } => match command.as_ref().unwrap_or(&UiCommand::Status) {
                UiCommand::Install(_) => "ui.install",
                UiCommand::Status => "ui.status",
                UiCommand::Url => "ui.url",
                UiCommand::Open => "ui.open",
            },
            Self::Mode { action, .. } => match action.as_ref().unwrap_or(&ApiModeCommand::Get) {
                ApiModeCommand::Get => "mode.get",
                ApiModeCommand::Set(_) => "mode.set",
            },
            Self::Sub { command } => command.canonical_action(),
            Self::Proxy { command } => command
                .as_ref()
                .map(ProxyCommand::canonical_action)
                .unwrap_or("proxy.list"),
            Self::System { action } => action.canonical_action(),
            Self::Tun { command } => command.canonical_action(),
            Self::Env { action } => action.canonical_action(),
            Self::Core { command } => command.canonical_action(),
            Self::Service { command } => command.canonical_action(),
            Self::Api { command } => command.canonical_action(),
            Self::Setup { command } => command.canonical_action(),
            Self::Update { command } => command.canonical_action(),
        }
    }

    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Contract => ActionSemantics::READ,
            Self::Ui { command } => match command.as_ref().unwrap_or(&UiCommand::Status) {
                UiCommand::Status | UiCommand::Url => ActionSemantics::READ,
                UiCommand::Install(_) | UiCommand::Open => ActionSemantics::WRITE,
            },
            Self::Mode { action, .. } => match action.as_ref().unwrap_or(&ApiModeCommand::Get) {
                ApiModeCommand::Get => ActionSemantics::READ,
                ApiModeCommand::Set(_) => ActionSemantics::WRITE,
            },
            Self::Sub { command } => command.semantics(),
            Self::Proxy { command } => command
                .as_ref()
                .map(ProxyCommand::semantics)
                .unwrap_or(ActionSemantics::READ),
            Self::System { action } => action.semantics(),
            Self::Tun { command } => command.semantics(),
            Self::Env { .. } => ActionSemantics::READ,
            Self::Core { command } => command.semantics(),
            Self::Service { command } => command.semantics(),
            Self::Api { command } => command.semantics(),
            Self::Setup { .. } => ActionSemantics::WRITE,
            Self::Update { command } => command.semantics(),
        }
    }

    pub fn machine_supported(&self) -> bool {
        match self {
            Self::Sub {
                command: ProfileCommand::Update(_),
            } => false,
            Self::Sub {
                command: ProfileCommand::Add(args),
            } if args.fetch => false,
            Self::Sub {
                command: ProfileCommand::Use(_),
            } => false,
            Self::Setup { .. } => false,
            Self::Ui {
                command: Some(UiCommand::Open),
            } => false,
            Self::Service {
                command: ServiceCommand::Log(args),
            } if args.follow => false,
            Self::Proxy {
                command:
                    Some(
                        ProxyCommand::Start(_)
                        | ProxyCommand::Stop(_)
                        | ProxyCommand::Status
                        | ProxyCommand::Auto { .. },
                    ),
            } => false,
            _ => true,
        }
    }

    pub fn validate_machine_inputs(&self) -> anyhow::Result<()> {
        use crate::machine::{ErrorCode, coded_error};
        match self {
            Self::Sub {
                command: ProfileCommand::Render(args),
            } if args.name.is_none() => Err(coded_error(
                ErrorCode::ExplicitInputRequired,
                "机器模式执行 `sub.render` 必须显式提供 --name；不要隐式依赖 active 订阅",
            )),
            Self::Sub {
                command: ProfileCommand::Render(args),
            } if args.output.is_some() => Err(coded_error(
                ErrorCode::UnsupportedMachineAction,
                "机器模式的 `sub.render` 只允许提交 runtime/config.yaml；自定义 --output 属于人类调试能力",
            )),
            Self::Sub {
                command: ProfileCommand::Validate(args),
            } if args.name.is_none() => Err(coded_error(
                ErrorCode::ExplicitInputRequired,
                "机器模式执行 `sub.validate` 必须显式提供 --name",
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Subcommand)]
pub enum ProxyCommand {
    #[command(about = "查看代理组与当前节点")]
    List(ApiCommonArgs),
    #[command(about = "切换代理组中的节点")]
    Switch(ApiProxySwitchArgs),
    #[command(about = "写入终端代理状态（默认端口来自 runtime 配置）", hide = true)]
    Start(StartArgs),
    #[command(about = "清理终端代理状态，可选移除 shell 自动启用钩子", hide = true)]
    Stop(StopArgs),
    #[command(about = "查看终端代理状态与自动启用状态", hide = true)]
    Status,
    #[command(about = "管理新终端自动启用代理（on/off/status）", hide = true)]
    Auto {
        #[command(subcommand)]
        action: AutoAction,
    },
}

impl ProxyCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::List(_) => "proxy.list",
            Self::Switch(_) => "proxy.switch",
            Self::Start(_) => "proxy.start",
            Self::Stop(_) => "proxy.stop",
            Self::Status => "proxy.status",
            Self::Auto { action } => match action {
                AutoAction::On { .. } => "proxy.auto.on",
                AutoAction::Off { .. } => "proxy.auto.off",
                AutoAction::Status { .. } => "proxy.auto.status",
            },
        }
    }

    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::List(_) | Self::Status => ActionSemantics::READ,
            Self::Auto {
                action: AutoAction::Status { .. },
            } => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
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
    #[command(about = "安装 systemd unit / launchd plist 并按需启用/启动")]
    Install(ServiceInstallArgs),
    #[command(about = "卸载 systemd unit / launchd plist，可选清理运行目录")]
    Uninstall(ServiceUninstallArgs),
    #[command(about = "启用开机自启")]
    Enable(ServiceTargetArgs),
    #[command(about = "关闭开机自启")]
    Disable(ServiceTargetArgs),
    #[command(about = "启动服务")]
    Start(ServiceTargetArgs),
    #[command(about = "停止服务")]
    Stop(ServiceTargetArgs),
    #[command(about = "重启服务")]
    Restart(ServiceTargetArgs),
    #[command(about = "查看服务状态")]
    Status(ServiceTargetArgs),
    #[command(about = "查看服务日志")]
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

#[derive(Subcommand, Clone)]
pub enum ProfileCommand {
    #[command(about = "添加订阅")]
    Add(ProfileAddArgs),
    #[command(about = "列出所有订阅与当前生效项")]
    List,
    #[command(about = "切换当前运行订阅（渲染 runtime，并按需重启服务）")]
    Use(ProfileUseArgs),
    #[command(about = "拉取指定订阅的最新内容（不渲染）")]
    Fetch(ProfileFetchArgs),
    #[command(about = "拉取订阅并渲染生效（fetch --force + 渲染 + 重启服务）")]
    Update(ProfileUpdateArgs),
    #[command(about = "删除订阅")]
    Remove(ProfileRemoveArgs),
    #[command(about = "将订阅渲染到运行配置 runtime/config.yaml")]
    Render(ProfileRenderArgs),
    #[command(about = "校验订阅 YAML 基础合法性")]
    Validate(ProfileValidateArgs),
    #[command(about = "管理 mixin.yaml 覆盖配置（show/set/unset/reset）")]
    Mixin {
        #[command(subcommand)]
        command: MixinCommand,
    },
}

#[derive(Subcommand)]
pub enum ApiCommand {
    #[command(about = "查看 external-controller 连接状态")]
    Status(ApiCommonArgs),
    #[command(about = "查看当前连接摘要")]
    Connections(ApiCommonArgs),
    #[command(about = "查看当前规则列表")]
    Rules(ApiCommonArgs),
    #[command(about = "查看运行配置")]
    Configs(ApiCommonArgs),
    #[command(about = "查看代理 Provider 列表")]
    Providers(ApiCommonArgs),
    #[command(about = "关闭所有活跃连接", name = "close-connections")]
    CloseConnections(ApiCommonArgs),
    #[command(about = "PATCH 修改运行配置", name = "config-patch")]
    ConfigPatch(ApiConfigPatchArgs),
    #[command(about = "获取当前流量快照")]
    Traffic(ApiCommonArgs),
    #[command(about = "获取最近日志快照")]
    Logs(ApiLogsArgs),
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
    #[arg(long, help = "订阅名称")]
    pub name: String,
    #[arg(long, help = "订阅 URL")]
    pub url: String,
    #[arg(
        long,
        help = "添加后立即拉取（人类便利选项；machine 请单独 sub fetch）"
    )]
    pub fetch: bool,
}

#[derive(Args, Clone)]
pub struct ProfileUseArgs {
    #[arg(long, help = "要切换为当前 runtime 的订阅名称")]
    pub name: String,
    #[arg(
        long,
        default_value = DEFAULT_SERVICE_NAME,
        help = "切换后联动重启的服务名"
    )]
    pub service_name: String,
    #[arg(long, help = "仅切换并渲染，不自动重启服务")]
    pub no_restart: bool,
}

#[derive(Args, Clone)]
pub struct ProfileUpdateArgs {
    #[arg(long, help = "订阅名称，默认当前 active")]
    pub name: Option<String>,
    #[arg(
        long,
        default_value = DEFAULT_SERVICE_NAME,
        help = "渲染后联动重启的服务名"
    )]
    pub service_name: String,
    #[arg(long, help = "仅渲染，不自动重启服务")]
    pub no_restart: bool,
}

#[derive(Args, Clone)]
pub struct ProfileFetchArgs {
    #[arg(long, help = "订阅名称")]
    pub name: String,
    #[arg(long, help = "忽略缓存强制更新")]
    pub force: bool,
}

#[derive(Args, Clone)]
pub struct ProfileRemoveArgs {
    #[arg(long, help = "订阅名称")]
    pub name: String,
}

#[derive(Args, Clone)]
pub struct ProfileRenderArgs {
    #[arg(long, help = "订阅名称，默认使用当前 active")]
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
    #[arg(long, help = "订阅名称，默认使用当前 active")]
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

impl Default for ApiCommonArgs {
    fn default() -> Self {
        Self {
            controller: None,
            secret: None,
            timeout_secs: 15,
        }
    }
}

#[derive(Args, Clone)]
pub struct ApiModeSetArgs {
    #[arg(value_enum, help = "目标模式")]
    pub mode: ApiModeValue,
}

#[derive(Args, Clone)]
pub struct ApiProxySwitchArgs {
    #[arg(long, help = "代理组名称")]
    pub group: String,
    #[arg(long, help = "目标代理节点名称")]
    pub proxy: String,
    #[command(flatten)]
    pub common: ApiCommonArgs,
}

#[derive(Args, Clone)]
pub struct ApiConfigPatchArgs {
    #[arg(long, help = "JSON 格式 payload，例如 '{\"mode\":\"rule\"}'")]
    pub data: String,
    #[command(flatten)]
    pub common: ApiCommonArgs,
}

#[derive(Args, Clone)]
pub struct ApiLogsArgs {
    #[arg(long, value_enum, help = "日志级别过滤")]
    pub level: Option<LogLevel>,
    #[command(flatten)]
    pub common: ApiCommonArgs,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
    Silent,
}

impl LogLevel {
    pub fn as_api_str(self) -> &'static str {
        match self {
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warning => "warning",
            LogLevel::Error => "error",
            LogLevel::Silent => "silent",
        }
    }
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
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "联动重启的服务名")]
    pub name: String,
    #[arg(long, help = "联动操作 user 级服务（LaunchAgent / systemd --user）")]
    pub user: bool,
    #[arg(long, help = "仅修改配置，不自动重启服务")]
    pub no_restart: bool,
}

#[derive(Args, Clone)]
pub struct TunStatusArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "用于检查状态的服务名")]
    pub name: String,
    #[arg(long, help = "检查 user 级服务（LaunchAgent / systemd --user）")]
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
    pub sub_url: String,
    #[arg(long, default_value = DEFAULT_SUB_NAME, help = "订阅名称")]
    pub sub_name: String,
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
        help = "将内核复制到该 PATH（服务启动仍使用 core 软链）"
    )]
    pub binary: PathBuf,
    #[arg(long, default_value = "/var/lib/clash-cli", help = "service 工作目录")]
    pub workdir: PathBuf,
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "服务名（systemd / launchd）")]
    pub service_name: String,
    #[arg(long, help = "初始化完成后不自动开启 tun")]
    pub no_tun: bool,
}

#[derive(Args, Clone)]
pub struct SetupUnifyArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "联动重启的服务名")]
    pub service_name: String,
    #[arg(long, help = "仅收敛 profile，不渲染与重启服务")]
    pub no_apply: bool,
    #[arg(long, help = "仅合并 profile，不替换历史目录为 /etc/clash-cli 软链接")]
    pub no_link: bool,
}

#[derive(Args, Clone)]
pub struct ServiceTargetArgs {
    #[arg(long, default_value = DEFAULT_SERVICE_NAME, help = "服务名（systemd / launchd）")]
    pub name: String,
    #[arg(long, help = "操作 user 级服务（LaunchAgent / systemd --user）")]
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

#[derive(Subcommand, Clone)]
pub enum SystemProxyAction {
    #[command(about = "开启桌面系统代理（GNOME gsettings / macOS networksetup）")]
    On,
    #[command(about = "关闭桌面系统代理并尽量恢复原设置")]
    Off,
    #[command(about = "查看系统代理是否由本工具开启")]
    Status,
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
    Fish,
}

impl ShellKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Fish => "fish",
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

impl CoreCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Install(_) => "core.install",
            Self::Upgrade(_) => "core.upgrade",
            Self::Version => "core.version",
            Self::Path => "core.path",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Version | Self::Path => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
}

impl ServiceCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Install(_) => "service.install",
            Self::Uninstall(_) => "service.uninstall",
            Self::Enable(_) => "service.enable",
            Self::Disable(_) => "service.disable",
            Self::Start(_) => "service.start",
            Self::Stop(_) => "service.stop",
            Self::Restart(_) => "service.restart",
            Self::Status(_) => "service.status",
            Self::Log(_) => "service.log",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Status(_) | Self::Log(_) => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
}

impl TunCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Doctor => "tun.doctor",
            Self::On(_) => "tun.on",
            Self::Off(_) => "tun.off",
            Self::Status(_) => "tun.status",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Doctor | Self::Status(_) => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
}

impl ProfileCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Add(_) => "sub.add",
            Self::List => "sub.list",
            Self::Use(_) => "sub.use",
            Self::Fetch(_) => "sub.fetch",
            Self::Update(_) => "sub.update",
            Self::Remove(_) => "sub.remove",
            Self::Render(_) => "sub.render",
            Self::Validate(_) => "sub.validate",
            Self::Mixin { command } => match command {
                MixinCommand::Show => "sub.mixin.show",
                MixinCommand::Set(_) => "sub.mixin.set",
                MixinCommand::Unset(_) => "sub.mixin.unset",
                MixinCommand::Reset => "sub.mixin.reset",
            },
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::List
            | Self::Validate(_)
            | Self::Mixin {
                command: MixinCommand::Show,
            } => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
}

impl ApiCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Status(_) => "api.status",
            Self::Connections(_) => "api.connections",
            Self::Rules(_) => "api.rules",
            Self::Configs(_) => "api.configs",
            Self::Providers(_) => "api.providers",
            Self::CloseConnections(_) => "api.close-connections",
            Self::ConfigPatch(_) => "api.config-patch",
            Self::Traffic(_) => "api.traffic",
            Self::Logs(_) => "api.logs",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::CloseConnections(_) | Self::ConfigPatch(_) => ActionSemantics::WRITE,
            _ => ActionSemantics::READ,
        }
    }
}

impl SetupCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Init(_) => "setup.init",
            Self::Unify(_) => "setup.unify",
        }
    }
}

impl SystemProxyAction {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::On => "system.on",
            Self::Off => "system.off",
            Self::Status => "system.status",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Status => ActionSemantics::READ,
            _ => ActionSemantics::WRITE,
        }
    }
}

impl EnvAction {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::On => "env.on",
            Self::Off => "env.off",
        }
    }
}

// --- Mixin 子命令 ---

#[derive(Subcommand, Clone)]
pub enum MixinCommand {
    #[command(about = "查看当前 mixin.yaml 内容")]
    Show,
    #[command(about = "按 YAML 点路径设置 mixin 字段")]
    Set(MixinSetArgs),
    #[command(about = "按 YAML 点路径删除 mixin 字段")]
    Unset(MixinSetArgs),
    #[command(about = "删除 mixin.yaml，恢复到无 mixin 状态")]
    Reset,
}

#[derive(Args, Clone)]
pub struct MixinSetArgs {
    #[arg(long, help = "YAML 点分路径，如 tun.enable 或 dns.enhanced-mode")]
    pub key: String,
    #[arg(
        long,
        default_value = "",
        help = "要设置的值（自动推导类型：bool/int/string）"
    )]
    pub value: String,
}

// --- Update 命令 ---

#[derive(Subcommand, Clone)]
pub enum UiCommand {
    #[command(about = "下载 metacubexd 到内核工作目录的 ui/")]
    Install(UiInstallArgs),
    #[command(about = "查看 Web UI 是否已安装及访问地址")]
    Status,
    #[command(about = "打印 Dashboard 地址")]
    Url,
    #[command(about = "尝试用系统浏览器打开 Dashboard")]
    Open,
}

#[derive(Args, Clone)]
pub struct UiInstallArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = MirrorSource::Auto,
        help = "下载镜像策略"
    )]
    pub mirror: MirrorSource,
    #[arg(long, help = "已安装也重新下载")]
    pub force: bool,
    #[arg(
        long,
        help = "内核工作目录（-d），UI 安装到 <workdir>/ui，默认 runtime 目录"
    )]
    pub workdir: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum UpdateCommand {
    #[command(about = "下载最新版本 CLI 并替换当前二进制")]
    Run(UpdateArgs),
    #[command(about = "仅检查是否有新版本，不执行更新")]
    Check(UpdateArgs),
}

#[derive(Args, Clone)]
pub struct UpdateArgs {
    #[arg(
        long,
        value_enum,
        default_value_t = MirrorSource::Auto,
        help = "下载镜像策略"
    )]
    pub mirror: MirrorSource,
    #[arg(long, help = "可选：预期的 SHA256（hex）用于校验下载资产")]
    pub sha256: Option<String>,
}

impl UpdateCommand {
    pub fn canonical_action(&self) -> &'static str {
        match self {
            Self::Run(_) => "update.run",
            Self::Check(_) => "update.check",
        }
    }
    pub fn semantics(&self) -> crate::machine::ActionSemantics {
        use crate::machine::ActionSemantics;
        match self {
            Self::Check(_) => ActionSemantics::READ,
            Self::Run(_) => ActionSemantics::WRITE,
        }
    }
}
