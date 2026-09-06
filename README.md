# clash-cli.rs

面向 Linux 与 macOS 的 mihomo/Clash 命令行工具（Rust）。

主要能力：
- `sub`：订阅管理与 runtime 合成
- `proxy`：查看代理组、切换节点
- `system`：桌面系统代理
- `tun`：TUN 诊断/启停/状态
- `mode`：出站模式
- `env`：当前终端代理环境变量
- `ui`：metacubexd 仪表板
- `core`：mihomo 内核安装/升级
- `service`：systemd / launchd 服务管理
- `api`：进阶 external-controller 接口
- `setup`：给人的一键初始化/收敛流程
- `update`：CLI 自更新

## 输出模型

人类 CLI 与机器接口明确分开：

- **默认永远是人类可读文本**。是否 TTY、是否被管道捕获，都不会偷偷改变协议。
- **机器调用必须显式加 `--machine`**，输出唯一的 Machine Contract v0。
- `--machine` 不自动 sudo、不弹交互、不依赖环境变量隐式开启。
- `setup *`、`ui open`、`service log --follow` 和隐藏的终端代理便利动作属于 human-only；agent 应组合原子命令。

查看机器契约：

```bash
clash --machine contract
```

示例：

```bash
clash --machine sub list
clash --machine sub render --name main
clash --machine proxy list
clash --machine system status
```

机器输出固定包含：`contract / ok / status / action / effect / data / error / meta`。详细定义见 `docs/机器契约-v0.md`。

## 系统要求

- Linux（systemd）或 macOS（launchd）
- 全局 TUN 需要 root/sudo
- 支持架构：`amd64`、`arm64`

## 安装并初始化

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- \
  --sub-url "https://example.com/sub.yaml"
```

网络访问 GitHub 不稳定时可指定镜像：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- \
  --mirror ghfast \
  --sub-url "https://example.com/sub.yaml"
```

仅安装 CLI：

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- --skip-setup
```

手动初始化：

```bash
# Linux，或 macOS 需要全局 TUN / LaunchDaemon
sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init --sub-url "https://example.com/sub.yaml"

# macOS 用户级 LaunchAgent，不接管全局流量
clash setup init --sub-url "https://example.com/sub.yaml" --no-tun
```

## 常用命令

```bash
clash ui
clash mode

clash sub list
clash sub add --name main --url "https://example.com/sub.yaml" --fetch
clash sub update --name main
clash sub use --name main
clash sub mixin set --key tun.enable --value true

clash proxy
clash proxy switch --group Proxy --proxy "香港"
clash system on
clash system status
clash tun on

eval "$(clash env on)"
```

`proxy start/stop/status/auto` 是给终端环境配置使用的人类便利命令，故意不属于 Machine Contract。

## 更新

```bash
clash update check
clash update run
```

下载的 CLI 和 mihomo 资产在安装前都会进行 SHA256 校验；缺少可信摘要时拒绝安装/更新。

## 开发

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test --test cli_harness
cargo test --test cli_integration
cargo test
```

测试不得真实执行 `tun on/off`、修改宿主系统代理或操作真实服务；这类路径使用纯函数或 mock 验证。更多说明见 `AGENTS.md`。

## 卸载

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/uninstall.sh | bash
```
