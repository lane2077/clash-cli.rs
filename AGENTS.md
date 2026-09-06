# AI 协作说明

面向 Linux 与 macOS 的 mihomo/Clash CLI（Rust）。改代码前先读本文、`docs/架构设计-v0.3.md` 和 `docs/机器契约-v0.md`。

## 最重要的接口规则

人类 CLI 与 Machine Contract 是两层不同接口：

- 不带 `--machine`：始终输出人类文本，不根据 TTY/管道自动改格式。
- 带 `--machine`：只输出 `clash.machine/v0` envelope。
- Machine Contract 必须显式开启；没有环境变量开关。
- machine 模式禁止自动 sudo、交互式一键编排、浏览器打开和无限日志流。
- canonical 命令只有 `sub / proxy / system / tun / mode / env / ui / core / service / api / update`；不要重新添加同义别名。
- `setup` 是 human-only orchestrator；agent 应调用原子能力。

Machine Contract 成功/失败都必须是单个 JSON 对象；业务模块只提供 `data`，`ok/action/status/effect/error/meta` 只能由 `src/machine.rs` / `src/output.rs` 生成。

## 验证

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
```

**测试不允许真实执行 TUN、修改系统代理或操作真实服务。** TUN 只测纯配置/状态函数；系统命令使用 mock。使用 `CLASH_CLI_HOME` 隔离状态。

## 目录

| 路径 | 职责 |
|---|---|
| `src/lib.rs` | 最外层分发；设置 canonical action / machine 语义 |
| `src/machine.rs` | Machine Contract v0、错误码、effect、契约自描述 |
| `src/output.rs` | machine envelope 唯一输出边界 |
| `src/cli.rs` | clap 类型、canonical action、machine capability/输入约束 |
| `src/profile.rs` | `sub`：订阅 + mixin 合成 runtime（唯一 YAML writer） |
| `src/api.rs` | mihomo HTTP：节点、mode、连接、配置等 |
| `src/proxy.rs` | 人类终端 env 状态，以及顶层 env/system 的实现 |
| `src/system_proxy.rs` | GNOME/macOS 系统代理适配器 |
| `src/tun/` | TUN overlay / doctor / status；数据面归 mihomo |
| `src/service.rs` | systemd / launchd 适配器 |
| `src/core.rs` | mihomo 下载、校验、版本软链 |
| `src/ui.rs` | Web UI |
| `tests/cli_harness.rs` | Machine Contract 与关键状态不变量 |
| `tests/harness_api.rs` | 无系统副作用的纯函数测试 |

## 状态模型

不要再把一个 `enabled` 当成所有状态。至少区分：

1. **期望状态**：用户/命令要求什么。
2. **已提交状态**：配置/index 已经写成什么。
3. **运行状态**：service/process 是否运行。
4. **观测状态**：接口/系统设置是否真的生效。

machine 的 `effect.state_changed` 表示命令是否改变了状态；`effect.verified` 表示结果是否被观测验证。写操作默认 `verified=false`，除非代码明确做了 postcondition 检查。

## 核心不变量

1. runtime 只允许合成管道写出：订阅 YAML + `profiles/mixin.yaml` → `runtime/config.yaml`。
2. 再次 render 不得冲掉 overlay 里的 TUN 策略。
3. `sub use/update` 失败不得留下 active/runtime 半状态。
4. 远端异常文本不得覆盖已有有效订阅/runtime。
5. systemd/launchd 的“命令执行”“已加载”“正在运行”不得混为一谈。
6. 默认 systemd ExecStart 指向 `core/mihomo` 软链。
7. doctor 遇到 Docker 网桥不得建议 `include-interface` / `exclude-interface`。
8. Machine Contract 不得靠错误文案推断关键错误；重要分支使用 `CodedError`。
9. machine 写操作若目标存在歧义，应要求显式输入，而不是静默取 active/default。

## 测试原则

- CLI machine 测试：`--machine` + 临时 `CLASH_CLI_HOME`。
- HTTP 测试显式指向 mock/不可用测试端口，不能碰真实 `127.0.0.1:9090`。
- 不允许 `run_from_args([..., "tun", "on/off", ...])` 这类有机会修改宿主系统的测试。
- 纯逻辑直接测生产 seam，不复制一份实现当 oracle。
