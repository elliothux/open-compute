# 行为差异

Queues 的 JavaScript API 与 Cloudflare 对齐；耐久性来自本机 `scheduler.sqlite`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| JavaScript API | [Queues JavaScript APIs](https://developers.cloudflare.com/queues/configuration/javascript-apis/) | 相同：`send` / `sendBatch`、`contentType`（json / text / bytes / v8）、`delaySeconds`、`metrics`、consumer `MessageBatch` / `ack` / `retry` |
| 耐久性 | 全球复制 | 本机 `scheduler.sqlite` |
| 投递 | at-least-once | at-least-once |
| 全局先进先出 | 提供 | 不提供 |
| 无法识别的 native dispatch | — | 可能保留该消息的 lease；后续投递可能使用同一 attempt 编号 |
| Pull consumer | 提供 | 不提供 |
| Binding | wrangler `queues` | producer `{ type, id, permissions? }`；consumer 为 Worker `queue` handler |

见[兼容性](/platform/compatibility)。
