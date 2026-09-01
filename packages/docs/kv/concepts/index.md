# 概念

每个 KV namespace 是一份独立的 SQLite 数据库（`data/kv/<account>/<namespace>/`）。键按 UTF-8 字节排序，不做 Unicode normalization。`put` 是单事务原子替换；失败时旧值仍可见。过期键在读和 list 路径上立即视为不存在。

`cacheTtl` 可以传入。本机没有边缘缓存：读到的是这份 SQLite 的当前值。

键最长 512 字节，不能为空、不能是 `.` 或 `..`。值最大 25 MiB。metadata 序列化后最大 1024 字节。`expirationTtl` 最小 60 秒。bulk `get` 最多 100 个键。`list` 默认/最大 1000。类型与错误形状与 [KV API](https://developers.cloudflare.com/kv/api/) 对齐。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [KV API](https://developers.cloudflare.com/kv/api/) | 相同：`put` / `get` / `getWithMetadata` / `list` / `delete`，text / json / arrayBuffer / stream、metadata、TTL、bulk get、list cursor |
| 复制 | Cloudflare 边缘网络 | 运行 `ocd` 的主机上的单节点 SQLite |
| `cacheTtl` | Colo cache | 接受该参数；无 colo cache |
| Jurisdictions | 提供 | 不提供 |
| REST bulk write / delete | 提供 | 不提供 |

下一步：[指南](/kv/guides/)。
