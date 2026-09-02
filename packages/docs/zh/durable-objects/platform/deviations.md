# 行为差异

Durable Objects 的 Worker / class API 与 Cloudflare 对齐；所有对象落在本地这一个 workerd。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | 相同：namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`、stub `fetch` / RPC、`state.storage` KV 与 SQL、transaction、output gate |
| 放置 | 地理调度，`locationHint` / jurisdiction / migration | 全部对象在本地这一个 workerd；`locationHint` / jurisdiction / migration 无地理效果 |
| Alarms | 提供 | 支持 7 个方法 |
| Hibernation | 提供 | 支持 |
| Binding | wrangler `durable_objects` | `{ type, id, className }`；`className` 必填 |
| `Fetcher.connect()` | 通用出网 | 绑定声明的连接 |

见[兼容性](/zh/platform/compatibility)。
