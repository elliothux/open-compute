# 行为差异

open-compute 上的 Vectorize 是单机精确索引，不是 Cloudflare 全球分布式 Vectorize 服务。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | 稳定后 beta 的 `Vectorize` | 公开方法相同 |
| 检索 | 托管近似拓扑 | 确定性精确检索 |
| 存储 | 托管 Vectorize | 每索引本机 SQLite；平台对象后端按需使用 Local/S3 |
| Beta `VectorizeIndex` | 遗留 | 不提供 |
| 全球就近存放 / 复制 | 提供 | 不提供 |
| 舰队级配额 | 托管套餐 | 本机创建时配额 |

见[兼容性](/zh/platform/compatibility)与 [Cloudflare Vectorize](https://developers.cloudflare.com/vectorize/)。
