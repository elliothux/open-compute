# 行为差异

KV 的 Worker API 与 Cloudflare 对齐；存储拓扑不同。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [KV API](https://developers.cloudflare.com/kv/api/) | 相同：`put` / `get` / `getWithMetadata` / `list` / `delete`，text / json / arrayBuffer / stream、metadata、TTL、bulk get、list cursor |
| 复制 | Cloudflare 边缘网络 | 运行 `ocd` 的主机上的单节点 SQLite |
| `cacheTtl` | Colo cache | 接受该参数；无 colo cache |
| Jurisdictions | 提供 | 不提供 |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding |
| REST bulk write / delete | 提供 | 不提供 |

见[兼容性](/zh/platform/compatibility)。
