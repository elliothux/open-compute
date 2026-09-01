# 行为差异

Workflows 的 binding / instance API 与 Cloudflare 对齐；执行权威是该节点上的本地 SQLite。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding / instance API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart |
| 执行 | 跨地域 | 该节点上的本地 SQLite |
| Callback | — | 结果提交前 at-least-once；replay 跳过已耐久完成的 callback |
| 外部副作用 | — | 不随 Workflow snapshot 回滚 |
| Dashboard / observability | 提供 | 不提供 |
| Binding | wrangler | `{ type, id, className }`；`className` 必填 |

batch / rollback / structured-clone / parallel 是实现行为。

见 [Compatibility](/platform/compatibility)。
