# 行为差异

Queues 的 JavaScript API 与 Cloudflare 对齐；耐久性来自该节点上的 `scheduler.sqlite`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、consumer `MessageBatch` / `ack` / `retry` |
| 耐久性 | 全球复制 | 该节点上的 `scheduler.sqlite` |
| 投递 | at-least-once | at-least-once |
| 全球 FIFO | 提供 | 不提供 |
| 未知 native dispatch | — | 可能保留 lease；后续投递可能重复同一 attempt number |
| Pull consumer | 提供 | 不提供 |
| Binding | wrangler `queues` | producer `{ type, id, permissions? }`；consumer 为 Worker `queue` handler |

见 [Compatibility](/platform/compatibility)。
