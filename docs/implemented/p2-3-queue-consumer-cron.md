# P2.3：Queue Consumer 与 Cron

> 状态：已实现并通过验收（2026-08-27），见 [P2.3 Gate 结果](./p2-3-gate-results.md)。

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- Queue batch eligibility／claim 在持久事务内确定，dispatch 使用 workerd 原生 queue custom event。
- Batch 与单 message 的 ack／retry、delay、attempt 和 dead-letter 由 completion authority 解释。
- 租约、generation 和 dispatch token 阻止 stale completion 修改当前 batch；transport unknown 不视为确定失败。
- 队列间公平性、并发、pause 和 shutdown 复用共享 scheduler kernel。
- Cron expression 在配置边界规范化；scheduler 根据 slot 身份处理 misfire、重启与重复触发。
- Scheduled handler 使用原生事件入口，部署与 generation 变化由持久 schedule authority 协调。
- 跨 control／scheduler 的遗漏由 reconciler 修复；snapshot／restore 保留状态身份与重放边界。

## 源码入口

- [`crates/storage/src/queue_consumers.rs`](../../crates/storage/src/queue_consumers.rs)
- [`crates/service/src/scheduler/queue.rs`](../../crates/service/src/scheduler/queue.rs)
- [`crates/service/src/scheduler/cron.rs`](../../crates/service/src/scheduler/cron.rs)
- [`packages/runtime/src/queues`](../../packages/runtime/src/queues)

## 验收依据

历史结论、实际命令与未验证项见[验收记录](p2-3-gate-results.md)。

当前测试入口与规则见[测试手册](../references/testing.md)。
