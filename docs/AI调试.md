# Machine Contract / AI 调试手册

AI、脚本、agent 调用统一使用显式 `--machine`。普通命令即使 stdout 被管道捕获也仍然是人类文本。

## 1. 隔离环境

```bash
export CLASH_CLI_HOME="$PWD/target/debug-home"
export CLASH_CLI_NO_AUTO_SUDO=1
mkdir -p "$CLASH_CLI_HOME"
```

不要把调试目录指向真实 `~/.config/clash-cli` 或 `/etc/clash-cli`。

## 2. 查看契约

```bash
cargo run -- --machine contract | jq .
```

## 3. 常用 machine 调试

```bash
cargo run -- --machine sub list | jq .
cargo run -- --machine core version | jq .
cargo run -- --machine ui status | jq .
cargo run -- --machine sub render --name main | jq .
```

成功应满足：退出 0、stdout 单个 JSON、`contract=clash.machine/v0`、`ok=true`。

失败应满足：非 0、stdout 单个 JSON、`ok=false`，并有稳定 `error.code`。

## 4. 两条测试路径

纯逻辑优先使用 `clash_cli::harness::*`，不依赖 systemd/launchd/TUN。

真实 CLI 契约使用：

```rust
run_with_home(&home, &["--machine", "sub", "list"]);
```

对应：`tests/cli_harness.rs`、`tests/cli_integration.rs`。

## 5. 安全要求

测试不得执行真实：

```text
tun on/off
system on/off
service start/stop/restart/install/uninstall
```

这类路径使用纯函数或 fake `launchctl/gsettings/systemctl`。HTTP 测试必须指定 mock/不可用测试端口，不能碰用户真实 controller。

## 6. Machine Contract 重点

- machine 只能由显式 `--machine` 开启；
- machine 不自动 sudo；
- 写操作有歧义时要求显式目标；
- `status=partial` 不等于成功；
- `effect.state_changed` 与 `effect.verified` 分开；
- 不根据 `error.message` 写控制逻辑，应使用 `error.code`。

完整定义见 `docs/机器契约-v0.md`。
