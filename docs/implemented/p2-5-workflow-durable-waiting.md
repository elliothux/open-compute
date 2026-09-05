# P2.5：Workflow 持久等待

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Durable yield 将等待状态写入 authority 并释放执行资源，wake 后依靠 step identity 与已提交结果重放。
- Retry、backoff、attempt timeout 和 workflow deadline 使用统一时钟与持久 fence。
- `sleep`／`sleepUntil` 保存唤醒条件；event inbox 和 wait／timeout 的竞争由事务决定唯一有效转移。
- Pause、resume、terminate 更新 instance generation，旧 dispatcher 与 completion 无权继续写入。
- 并行 step 使用有界调度和独立结果 identity，不能把取消或响应丢失等同于副作用不存在。
- Retention、artifact 引用、删除与 ID 重用在重启后仍隔离旧代；snapshot 还原完整等待与事件状态。
- 当前 API／类型与支持范围见维护矩阵，不保留历史 Capability V1/V2 双模型。

## 源码入口

- [`crates/storage/src/workflows.rs`](../../crates/storage/src/workflows.rs)
- [`crates/workers/src/workflow_lifecycle.rs`](../../crates/workers/src/workflow_lifecycle.rs)
- [`packages/runtime/src/workflows`](../../packages/runtime/src/workflows)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p2-5-gate-results.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
