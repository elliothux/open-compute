# 行为差异

Workflows 的 binding / instance API 与 Cloudflare 对齐；步骤状态存在本机 SQLite。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding / instance API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart |
| 执行 | 跨地域 | 本机本地 SQLite |
| Callback | — | 结果提交前可能重复执行；replay 跳过已经落盘的步骤回调 |
| 外部副作用 | — | 不随 Workflow snapshot 回滚 |
| Dashboard / observability | 提供 | 不提供 |
| Binding | wrangler | `{ type, id, className }`；`className` 必填 |

batch / rollback / structured-clone / parallel 是实现行为。

见[兼容性](/platform/compatibility)。
