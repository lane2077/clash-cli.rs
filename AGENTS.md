# AI 协作说明

面向 Linux 与 macOS 的 mihomo/Clash CLI（Rust）。改代码前先读本文和 `docs/AI调试.md`。

终端默认中文；**非 TTY（脚本、管道、agent 子进程）默认 JSON**，不必加 `--json`。成功 `ok: true`；失败非零退出且 JSON 含 `error`。`env on/off`（以及旧的 `proxy env`）/ `service log -f` / `setup *` 例外：stdout 保持脚本、日志或初始化进度流。强制中文用 `--text`，强制 JSON 用 `--json`。用 `CLASH_CLI_HOME` 隔离状态。

日常命令对齐 Clash Verge 菜单：`sub`（订阅，`profile` 是别名）、`proxy`（代理组/节点）、`system`（系统代理）、`tun`、`mode`（出站模式）、`env`（复制环境变量）、`ui`（仪表板）。终端 HTTP `proxy start/stop/auto` 仍可用，但不出现在 `proxy --help` 的主说明里。

## 验证

```bash
cargo test
```

测试本身就是 harness：隔离 `CLASH_CLI_HOME`、拉起真实 `clash` 二进制、断言 JSON 契约。不要再包一层 shell 入口。

## 目录

| 路径 | 职责 |
|------|------|
| `src/lib.rs` | 薄分发：Verge 动词进对应域；`clash_cli::harness` 给测试/AI 的纯函数 |
| `src/main.rs` | 薄二进制 |
| `src/cli.rs` | clap 类型；`ProxyCommand::into_node_api` 把代理组从终端 env 拆开 |
| `src/profile.rs` | `sub`/`profile`：订阅 + mixin 合成 runtime（唯一 YAML writer） |
| `src/api.rs` | mihomo HTTP：`proxy list/switch`、`mode`、`api *` |
| `src/proxy.rs` | 终端 env / `proxy start` / 桌面 `system`（不写 runtime YAML、不调 HTTP） |
| `src/system_proxy.rs` | GNOME/macOS 系统代理命令 |
| `src/tun/` | tun overlay / doctor / status；只改 mixin 再走 profile 合成 |
| `src/service.rs` | systemd unit / launchd plist；默认启动 `core/mihomo` 软链 |
| `tests/common/mod.rs` | 隔离 home、跑二进制、断言 JSON |
| `tests/fixtures/` | 订阅 YAML 样例 |
| `tests/harness_api.rs` | 直接调库函数 |
| `tests/cli_harness.rs` | 真实 CLI 契约（含失败 JSON） |

## 平台

支持 **Linux 与 macOS**。Windows 仍不支持。

- Linux：systemd + `/dev/net/tun` + 可选 nft auto-redirect
- macOS：launchd + mihomo utun/auto-route；TUN 通常需要 sudo
- 不要把 macOS 上的失败当成「仅支持 Linux」

## 模块（深：小接口后面藏行为）

改代码时用这些词：模块、接口、seam、适配器、深度。不要为测试再抽一层过路函数。

- **订阅生效** seam：`apply_subscription`（拉取 / 合成 runtime / 重启）。`sub use --apply` 与 `sub update` 都穿过它。测试也穿过它。
- **合成** seam：`merge_subscription_overlay` / `render_runtime_from_home`。`tun on/off` 只改 mixin，再走合成。
- **节点 vs 终端 env** seam：`ProxyCommand::into_node_api`。systemd 与 launchd 是服务生命周期的两个适配器，plist/unit 生成函数是接口。

## 不变量（回归时优先查）

1. **runtime 只允许合成管道写出**：订阅 YAML + `profiles/mixin.yaml` → `runtime/config.yaml`。`tun on/off` 只改 mixin，再 `render`。
2. **再次 render 不得冲掉 overlay 里的 `tun.enable` / `tun.auto-redirect`。**
3. **默认 systemd ExecStart 指向 `core/mihomo` 软链**，不是 `/usr/local/bin/mihomo` 拷贝。
4. **tun 实际状态**不要求 CLI 自建 `clash_cli_tun` / `CLASH_CLI_TUN` 表；数据面归 mihomo `auto-redirect`。
5. **doctor 遇到 Docker 网桥不得建议 `include-interface` / `exclude-interface`。**

## 怎么加测试

- 纯逻辑：在 `tests/harness_api.rs` 调 `clash_cli::harness::*`，或在对应模块的 `#[cfg(test)]` 里调同一函数。禁止再实现一份 merge 规则当 oracle。
- CLI：用 `tests/common` 的 `temp_home` + `run_with_home`，设 `CLASH_CLI_HOME`，并加 `CLASH_CLI_NO_AUTO_SUDO=1`。
- 不要把配置目录指到临时 scratch 以外的真实 `~/.config`。
- 管道里失败形态必须是 `{"ok": false, "error": "..."}`（不必加 `--json`）。

## 常用命令

```bash
cargo test
cargo test --test harness_api
cargo test --test cli_harness
CLASH_CLI_HOME="$PWD/target/debug-home" cargo run -- sub list
CLASH_CLI_HOME="$PWD/target/debug-home" cargo run -- --text profile list
```
