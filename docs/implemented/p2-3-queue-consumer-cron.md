# P2.3：Queue Consumer 与 Cron

状态：**verified（2026-08-27）**。

## 最终结果

- Queue batch eligibility 和 claim 在持久事务中确定，workerd 原生 queue event 执行 handler。
- Batch／message ack、retry、delay、attempt 和 dead-letter 由 completion authority 解释；lease、generation 和 token 阻止 stale completion。
- Timeout 或 transport abort 保持 Unknown，不被当作确定失败立即重放。
- Scheduler 统一处理队列公平性、并发、pause、shutdown、Cron slot、misfire 和 restart。
- Cron expression 在配置边界规范化；scheduled handler 使用原生 event，并固定 deployment generation。
- Reconciler 和 snapshot/restore 保留跨 control／scheduler 的身份与重放边界。

当前支持面见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 历史验证

基线 revision 为 `fd3362036c5e8e60cb9d5ad4ee2c55c6b7a8f542`，workerd 为 `v1.20260826.1`。
Queue／Cron real-runtime Gate、workspace、静态检查及 coverage 成功；当日 Gate 三轮均通过，coverage 为 90.03%。
当前运行轮数以[测试手册](../references/testing.md)为准。
