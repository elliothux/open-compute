# 概念

Workflow definition 是 catalog 资源。实例与 step 状态存储在本机 SQLite。`run` 重放时，已提交的 `step.do` 不再执行 callback；未提交的 callback 在提交前可能重复执行。

对 KV / R2 / 外部 HTTP 的副作用**不会**随 Workflow snapshot 回滚。外部写入应做成幂等。

不提供跨区域 placement，也不提供 Cloudflare dashboard 中的 Workflow 可观测性。

[Workflows](https://developers.cloudflare.com/workflows/) 的 class / `step.do` / instance handle 与 Cloudflare 对齐。有界并行 `step.do`（默认最多 4 路）是实现行为。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Workflows](https://developers.cloudflare.com/workflows/) | 相同：class / `step.do` / instance handle |
| 执行 | 跨地域 | 本机 SQLite |
| Callback | — | 提交前可能重复执行；已完成的 callback 在 replay 时跳过 |
| 外部副作用 | — | 不随 snapshot 回滚 |
