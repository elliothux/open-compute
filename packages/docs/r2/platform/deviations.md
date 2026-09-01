# 行为差异

R2 的 Worker API 与 Cloudflare 对齐；对象字节落在配置的 S3-compatible provider。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、multipart、HTTP metadata |
| 对象字节 | Cloudflare R2 存储 | 配置的 S3-compatible provider |
| 全球 placement | 提供 | 不提供 |
| r2.dev 公开产品 | 提供 | 不提供 |
| Jurisdictional restrictions | 提供 | 不提供 |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding 或 provider 的 S3 API |

见[兼容性](/platform/compatibility)。
