# TUN 设计 v0.3

## 1. 原则

TUN 是高副作用能力。CLI 负责**策略、诊断和观测**，mihomo 负责实际数据面；测试环境不得因为验证 CLI 而修改宿主网络。

## 2. 配置路径

唯一写入链路：

```text
clash tun on/off
  -> profiles/mixin.yaml
  -> sub render 共用合成 seam
  -> runtime/config.yaml
```

`runtime/config.yaml` 不允许被 TUN 模块单独手写。再次 `sub render` 必须保留 overlay 中的 TUN 策略。

## 3. 平台差异

Linux：
- `/dev/net/tun`；
- `CAP_NET_ADMIN + CAP_NET_RAW`；
- mihomo `auto-route/auto-redirect`；
- CLI 只清理历史遗留的自建表，不创建新的 PREROUTING 数据面。

macOS：
- utun 编号动态分配；
- `auto-route`；
- 仅凭 launchd job 存在不能证明 TUN 已接管；无法确认接口归属时 machine 返回 unknown/null，而不是猜 true。

## 4. 状态模型

`tun status` 分开报告：

- committed：runtime 中 `tun.enable` 等配置；
- runtime：关联 service 是否正在运行；
- observed：TUN 接口/平台可观测事实；
- history：最近一次 `runtime/tun.state` 操作记录。

`actual_ok` 只有在所需观测证据充分时才应为 true；证据不足时是 unknown，而非“最好猜一个”。

## 5. doctor

检查：设备、权限、路由能力、sysctl、runtime 配置与 Docker 网桥环境。

Docker 网桥只用于诊断提示，不自动生成 `include-interface` / `exclude-interface`。

## 6. Machine Contract

- `tun.doctor` / `tun.status`：read action；
- `tun.on` / `tun.off`：write action；
- machine 永不自动 sudo；权限不足返回 `PERMISSION_REQUIRED`；
- 写操作成功默认 `effect.verified=false`，调用方应随后执行 `tun.status` 获取观测结果。

## 7. 测试安全

自动测试只允许：

- overlay 纯函数；
- YAML 合成；
- 状态判定纯函数；
- mock system/service 输出。

禁止测试直接执行真实 `tun on/off`。端到端 TUN 验收只在允许断网的独立 Linux 验收机人工执行。
