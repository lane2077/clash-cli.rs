# clash-cli.rs

面向 Linux 的 Clash 命令行工具（Rust）。

主要能力：
- `setup init`：一键初始化（内核 + 订阅 + 渲染 + service + tun）
- `proxy`：终端代理开关与自动注入
- `core`：mihomo 内核安装/升级
- `service`：systemd 管理
- `tun`：诊断/启停/状态
- `profile`：订阅管理与渲染
- `api`：external-controller 查询与操作

## 系统要求
- Linux（systemd）
- `curl`、`tar`
- root/sudo 权限
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
sudo env CLASH_CLI_HOME=/etc/clash-cli clash setup init --profile-url "https://example.com/sub.yaml"
```

## 常用命令

```bash
clash --help
clash setup init --help
clash tun status --name clash-mihomo
clash api ui-url
clash proxy start
eval "$(clash proxy env on)"
```

## 一键卸载

```bash
curl -fsSL https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/uninstall.sh | bash
```

如果 `raw.githubusercontent.com` 不稳定，也可走镜像：

```bash
curl -fsSL https://ghfast.top/https://raw.githubusercontent.com/lane2077/clash-cli.rs/main/scripts/uninstall.sh | bash
```
