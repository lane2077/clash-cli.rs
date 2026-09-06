# clash-cli.rs

面向 Linux 与 macOS 的 Clash 命令行工具（Rust）。

主要能力：
- `setup init`：一键初始化（内核 + 订阅 + 渲染 + service + tun）
- `sub`：订阅（`profile` 仍是别名）
- `proxy`：查看代理组并切换节点
- `system`：桌面系统代理
- `tun`：TUN 诊断/启停/状态
- `mode`：出站模式
- `env`：复制终端环境变量
- `ui`：仪表板
- `core`：mihomo 内核安装/升级
- `service`：systemd（Linux）/ launchd（macOS）管理
- `api`：进阶 external-controller 接口
- `update`：CLI 自身版本更新

## 系统要求
- Linux（systemd）或 macOS（launchd）
- 全局 TUN 需要 root/sudo
- 支持架构：`amd64`、`arm64`

## 一键安装并初始化（推荐）

把下面命令里的订阅地址替换成你的：

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- \
  --profile-url "https://example.com/sub.yaml"
```

如果你网络访问 GitHub 不稳定，可指定镜像：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- \
  --mirror ghfast \
  --profile-url "https://example.com/sub.yaml"
```

## 仅安装 CLI（二进制）
```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- --skip-setup
```

安装后手动初始化：

```bash
# Linux（或 macOS 需要全局 TUN / LaunchDaemon）
sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init --profile-url "https://example.com/sub.yaml"

# macOS 用户级 LaunchAgent（不接管全局流量）
clash setup init --profile-url "https://example.com/sub.yaml" --no-tun
```

## 更新 CLI

```bash
# v0.2.0+ 内置自更新
clash update check           # 检查最新版本
clash update run             # 下载并替换

# 旧版本通过安装脚本升级
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/install.sh | bash -s -- --skip-setup
```

## 怎么用（人看中文，管道自动 JSON）

终端里直接跑，输出中文。被脚本/管道调用时自动变成 JSON，不必加 `--json`。只有在终端里想看 JSON 才用 `--json`；管道里想看中文用 `--text`。

`eval "$(clash env on)"` 始终是 shell 脚本。

```bash
# 和 Clash Verge 菜单同一套词
clash ui                                        # 仪表板
clash mode                                      # 出站模式（规则/全局/直连）
clash sub list
clash sub add --name main --url "https://example.com/sub.yaml" --use-profile
clash sub update                                # 拉取最新并生效
clash sub use --name main --apply               # 切换订阅并生效
clash proxy                                     # 代理组与当前节点
clash proxy switch --group Proxy --proxy "香港"
clash system on                                 # 桌面系统代理（浏览器等）
clash tun on                                    # 全局接管
eval "$(clash env on)"                          # 仅当前终端

# 本地覆盖订阅原文
clash sub mixin set --key tun.enable --value true

# 旧命令仍可用：profile / api proxies / api proxy-switch / proxy env / proxy system
```

## 开发调试

```bash
cargo test
```

说明见 `AGENTS.md`。

## 一键卸载

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/uninstall.sh | bash
```

如果 `raw.githubusercontent.com` 不稳定，也可走镜像：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/uninstall.sh | bash
```
