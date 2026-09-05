# P2.2：Queue Producer

> 状态：已实现并完成本地 Exit Gate；结论为 Conditional Go，详见

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Queue catalog、账号身份和生命周期由控制库管理；消息与调度状态属于 queue／scheduler authority。
- Deployment binding 固定 Queue 身份与 producer 权限，private transport 再验证调用 scope。
- Serialization 在运行时边界完成，二进制数据不通过 JSON 隐式改变类型；公开 body／batch 上限以当前合同为准。
- Producer 接收只有在持久提交后才能确认；commit 后响应丢失保留未知结果语义。
- 跨库 lifecycle、资源引用和删除由持久状态与 reconciler 收敛；stale generation 无法向新 Queue 写入。
- Native output gate、DO producer 支持及错误形状遵循当前 compatibility，历史 Conditional Go 不扩大支持范围。
- 队列容量、retention、metrics 和维护任务复用 scheduler kernel，不维护第二套调度器。

## 源码入口

- [`crates/storage/src/queues`](../../crates/storage/src/queues)
- [`crates/workers/src/queue_lifecycle.rs`](../../crates/workers/src/queue_lifecycle.rs)
- [`crates/service/src/queue_backend.rs`](../../crates/service/src/queue_backend.rs)
- [`packages/runtime/src/queues`](../../packages/runtime/src/queues)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p2-2-results.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
