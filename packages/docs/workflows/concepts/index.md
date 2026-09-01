# 概念

Workflow definition 是 catalog 资源。实例与 step 状态在本地 SQLite。`run` 被重放时，已经提交的 `step.do` 不会再执行 callback；未提交的 callback 可能再跑一次（at-least-once until commit）。

对 KV / R2 / 外部 HTTP 的副作用**不会**随 Workflow snapshot 回滚。把外部写做成幂等。

不提供跨区域 placement，也不提供 Cloudflare dashboard 里的 Workflow 观察面。

[Workflows](https://developers.cloudflare.com/workflows/) 的 class / `step.do` / instance handle 与 Cloudflare 对齐。有界并行 `step.do`（默认最多 4 路）是实现行为。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Workflows](https://developers.cloudflare.com/workflows/) | 相同：class / `step.do` / instance handle |
| 执行 | 跨地域 | 该节点上的本地 SQLite |
| Callback | — | 提交前 at-least-once；已完成的 callback 在 replay 时跳过 |
| 外部副作用 | — | 不随 snapshot 回滚 |
