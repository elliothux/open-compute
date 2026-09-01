# 偏差

登记 ID：**`OC-WORKFLOW-001`**。

Workflow 在本地 SQLite authority 上执行。callback 在结果提交前是 at-least-once；replay 会跳过已耐久完成的 callback；外部产品副作用不会随 Workflow snapshot 回滚。不声称跨地域执行、全球 placement 或 Cloudflare dashboard/observability。

72 个目标成员因此是 `supported_with_deviation`。batch / rollback / structured-clone / parallel 是实现行为，不是偏差。

见 [Compatibility](/platform/compatibility) 与 `docs/references/p1-deviations.md`。
