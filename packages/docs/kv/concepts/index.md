# 概念

每个 KV namespace 是一份独立的 SQLite 数据库（`data/kv/<account>/<namespace>/`）。键按 UTF-8 字节排序，不做 Unicode normalization。`put` 是单事务原子替换；失败时旧值仍可见。过期键在读和 list 路径上立即视为不存在。

`cacheTtl` 可以传，但本机没有 colo cache：读到的就是这份 SQLite 当前值。

## 与 Cloudflare 相同

键最长 512 字节，不能为空、不能是 `.` 或 `..`。值最大 25 MiB。metadata 序列化后最大 1024 字节。`expirationTtl` 最小 60 秒。bulk `get` 最多 100 个键。`list` 默认/最大 1000。类型与错误形状跟 [KV API](https://developers.cloudflare.com/kv/api/) 对齐。

## 故意不同

**`OC-KV-001`**：单节点 SQLite 权威，不声称全球复制或传播时延。没有 Cloudflare 的 eventual consistency 窗口可测，因为根本没有第二份副本。没有 jurisdiction。没有 REST bulk write/delete 产品。

下一步：[指南](/kv/guides/)。
