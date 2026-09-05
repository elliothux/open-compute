# 行为差异

R2 的 Worker API 与 Cloudflare 对齐，包括准确的 single/part/multipart ETag 语义及 lowercase-hex `ssecKeyMd5`；对象字节落在单节点配置的 Local 或 S3 authority。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、multipart、HTTP metadata |
| 对象字节 | Cloudflare R2 存储 | 单节点配置的 Local 或 S3 authority |
| 全球 placement | 提供 | 不提供 |
| r2.dev 公开产品 | 提供 | 不提供 |
| Jurisdictional restrictions | 提供 | 不提供 |
| REST / `/client/v4` | 提供 | 兼容 bucket 与 object 操作 |

见[兼容性](/zh/platform/compatibility)。
