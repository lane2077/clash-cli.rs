# clash-cli

面向 Linux 的 Clash 命令行工具（Rust）。

当前已实现：
- `proxy`：当前终端临时代理 + 新终端自动代理。
- `core`：mihomo 内核下载、升级、版本与路径查询（镜像支持 `ghfast` 回退）。
- `service`：systemd unit 安装、卸载、启停、状态与日志。
- `tun`：`doctor` 诊断 + `on/off/status`（已支持 nft/iptables 规则下发与清理、service 联动重启）。
- `profile`：订阅 profile 管理、拉取、渲染、校验。
- `api`：external-controller 查询与模式切换。
- `setup`：`setup init` 一键初始化（内核 + 订阅 + 渲染 + service + tun）。
- 全局 `--json`：主要子命令支持结构化输出，失败场景同样返回 JSON（`ok=false`）；`setup init` 当前仅支持文本输出。

默认策略（v0.2）：
- `profile render` 默认覆盖本地监听入口（`mixed-port=7890`、`socks-port=7891`、`external-controller=127.0.0.1:9090`、`bind-address=127.0.0.1`、`allow-lan=false`）。
- `profile render` 默认启用 Dashboard（`external-ui=ui`、`external-ui-name=metacubexd`、`external-ui-url` 指向 `metacubexd`）。
- 如需保留订阅里的监听端口，可用 `clash profile render --follow-subscription-port`。
- `proxy start` 未显式传端口时，优先读取 `runtime/config.yaml`（避免与运行配置端口不一致）。

设计文档：
- `docs/架构设计-v0.2.md`
- `docs/tun设计-v0.2.md`
- `docs/Linux验收清单-v0.1.md`

## 快速示例
```bash
clash proxy start
eval "$(clash proxy env on)"

clash proxy auto on --shell zsh

clash core install --version latest --mirror auto
clash core version

clash service install
clash service status
clash service log -f
clash service uninstall --purge

clash tun doctor
clash tun on
clash tun off
clash tun status

clash profile add --name main --url https://example.com/sub.yaml --use-profile
clash profile render

clash api status
clash api mode get
clash api mode set rule
clash api ui-url

sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init \
  --profile-url https://example.com/sub.yaml

scripts/install.sh --profile-url https://example.com/sub.yaml
scripts/uninstall.sh

clash --json profile list
```
