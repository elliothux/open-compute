# P2.5：Workflow 持久等待

状态：**implemented / Conditional Go；P2 Exit PASS（2026-08-28）**。

## 最终结果

- `sleep`、`sleepUntil`、event wait、retry/backoff 和 attempt timeout 持久化后释放执行资源，wake 时按 step identity replay。
- Event inbox、timeout 和 pause／resume／terminate 的竞争由事务决定唯一状态转移，旧 dispatcher 或 completion 被 generation fence 拒绝。
- Restart 保留原 input、version 和 internal identity，只推进 generation；retention 与 purge 在重启后安全收敛。
- Parallel `step.do` 有界，已提交结果独立复用；取消或响应丢失不代表外部副作用未发生。
- Snapshot/restore 保留 waiting、paused、inbox、deadline、artifact refs 和旧代隔离。
- 当前模型只有一套 capability，不保留历史 V1/V2 运行路径。

## 历史验证与限制

Darwin arm64、workerd `v1.20260826.1` 上，684 个 workspace tests、P2.2–P2.5、P2 Exit、静态检查和 coverage
成功；coverage 为 56,391 / 62,547 Rust lines（90.16%）。P2 Exit 覆盖 Queue→Consumer→Workflow→KV/R2/D1/DO
链路和 14 个 Workflow SIGKILL 边界。

默认单值上限 1 MiB、step timeout 60 秒、并行度 4。外部副作用仍需业务幂等；DO 内 Workflow mutation 不支持，
也不提供任意 Promise DAG、rollback hook 或 exactly-once。
