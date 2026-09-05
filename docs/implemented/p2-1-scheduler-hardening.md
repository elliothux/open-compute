# P2.1：共享 Scheduler 内核

> 状态：已实现并验证（2026-08-26）

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Typed workload 定义 claim、dispatch、completion 和 retry 所需的领域输入；kernel 不读取产品私有状态。
- Account／workload admission、公平调度和并发预算由统一 kernel 管理。
- Batch claim 是短事务，网络、workerd dispatch 和等待在事务外执行。
- 单一 Clock 和 deadline 模型管理唤醒；墙钟跳变、虚拟时间与重启都必须有确定行为。
- Retry、backoff 和 circuit breaker 不改变租约归属；未知结果保留必要 fence，禁止重复确认。
- Projection generation 与 owner lifecycle 拒绝 stale completion；pause／shutdown 不丢失已持久化任务。
- 产品 handler、operator 控制与 metrics 按 ownership 分层，异常只影响对应 workload。

## 源码入口

- [`crates/storage/src/scheduler.rs`](../../crates/storage/src/scheduler.rs)
- [`crates/service/src/scheduler`](../../crates/service/src/scheduler)

## 验收依据

以下保留原阶段验证记录，命令与轮数不作为当前执行要求：

> 验证证据：`./test/test-p2-1.sh` 聚合 Gate 通过；`./test/coverage.sh` 通过，
> workspace Rust line coverage 为 90.07%。G0 维持既有精确 `D-abort` allowlist 下的
> Conditional Go，未扩大接受范围。
>

当前测试入口与规则见[测试手册](../references/testing.md)。
