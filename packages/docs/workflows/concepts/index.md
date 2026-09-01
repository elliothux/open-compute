# 概念

Workflow definition 是 catalog 资源。实例与 step 状态在本地 SQLite。`run` 被重放时，已经提交的 `step.do` 不会再执行 callback；未提交的 callback 可能再跑一次（at-least-once until commit）。

对 KV / R2 / 外部 HTTP 的副作用**不会**随 Workflow snapshot 回滚。把外部写做成幂等。

没有跨区域 placement，也没有 Cloudflare dashboard 里的 Workflow 观察面（`OC-WORKFLOW-001`）。

## 与 Cloudflare 相同

[Workflows](https://developers.cloudflare.com/workflows/) 的 class / `step.do` / instance handle。有界并行 `step.do`（默认最多 4 路）是实现行为，不是偏差。

## 故意不同

**`OC-WORKFLOW-001`**：本地 SQLite；callback at-least-once until commit；外部副作用不回滚；无跨地域、无 CF dashboard/observability。
