# clash-cli Linux 验收清单 v0.3

这份清单用于**独立 Linux 验收机**。其中 service/TUN 步骤会修改系统状态，不应在日常开发机、远程生产机或网络敏感设备上运行。

## 1. 编译与 Machine Contract

```bash
cargo fmt --check
cargo check --all-targets
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --machine contract | jq .
```

预期：
- 契约为 `clash.machine/v0`；
- machine stdout 是单个 JSON 对象；
- human CLI 不因管道而自动切换 JSON。

## 2. 订阅原子流程

```bash
export CLASH_CLI_HOME="$PWD/target/accept-home"

cargo run -- --machine sub add --name main --url "你的订阅URL" | jq .
cargo run -- --machine sub fetch --name main --force | jq .
cargo run -- --machine sub use --name main | jq .
cargo run -- --machine sub render --name main | jq .
cargo run -- --machine sub validate --name main | jq .
```

预期：
- 每步只有一个明确 effect；
- 非法订阅不会覆盖旧文件/runtime；
- 未拉取订阅不能设为 active；
- runtime 包含本地安全监听默认值。

## 3. Core

```bash
cargo run -- core install --version latest --mirror auto
cargo run -- --machine core version | jq .
cargo run -- --machine core path | jq .
```

预期：下载资产必须通过可信 SHA256 校验；无摘要或不匹配时拒绝安装。

## 4. Controller / Proxy / Env

```bash
cargo run -- --machine proxy list | jq .
cargo run -- --machine mode get | jq .
cargo run -- --machine env on | jq .
eval "$(cargo run -- env on)"
```

预期：
- machine 从 runtime 获取 controller/代理端口，不静默猜 9090/7890；
- 没有 runtime 时返回 `EXPLICIT_INPUT_REQUIRED` 或 `RUNTIME_CONFIG_REQUIRED`；
- human `env on` 仍输出可直接 eval 的 shell 脚本。

## 5. Service（破坏性，独立验收机）

```bash
sudo cargo run -- service install --name clash-mihomo
sudo cargo run -- --machine service status --name clash-mihomo | jq .
sudo cargo run -- service restart --name clash-mihomo
sudo cargo run -- service log --name clash-mihomo -n 50
```

预期：
- `core/mihomo` 软链是 ExecStart；
- “已加载”和“正在运行”分开判定；
- systemctl 非零退出不会被吞掉。

## 6. TUN（高风险，只在允许断网的独立 Linux 验收机）

执行前确认可接受临时网络中断，并准备本地控制台/回滚路径。

```bash
sudo cargo run -- tun doctor
sudo cargo run -- tun on --name clash-mihomo
sudo cargo run -- --machine tun status --name clash-mihomo | jq .
```

预期：
- TUN 策略写入 `profiles/mixin.yaml`，再通过统一合成管道进入 runtime；
- Docker 网桥不会触发 `include-interface` / `exclude-interface` 建议；
- 数据面由 mihomo `auto-route/auto-redirect` 管理，不要求 CLI 自建 nft/iptables 表；
- machine `tun status` 区分配置、service、接口实际观测状态。

回滚：

```bash
sudo cargo run -- tun off --name clash-mihomo
sudo cargo run -- --machine tun status --name clash-mihomo | jq .
```

## 7. 清理

```bash
sudo cargo run -- service uninstall --name clash-mihomo --purge
rm -rf "$CLASH_CLI_HOME"
```
