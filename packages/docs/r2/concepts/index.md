# 概念

逻辑 bucket 是平台 catalog 里的资源。对象字节落在配置的 S3-compatible provider 上（`r2_prefix` 与平台 `prefix` 必须不相交）。Worker 通过 binding 看到 Cloudflare R2 那一套方法。同一份字节也可以用 provider 的 S3 API 读写——那是存储协议，不是 Cloudflare 管理面。

不提供全球 placement。对象位于配置的那一个 endpoint / bucket。

`put` / `get` / `head` / `delete` / `list`、range、条件（etag / 时间）、checksum、multipart、custom metadata、HTTP metadata 与 [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) 对齐。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`put` / `get` / `head` / `delete` / `list`、range、条件写、checksum、multipart、metadata |
| 对象字节 | Cloudflare R2 存储 | 配置的 S3-compatible provider |
| 全球 placement | 提供 | 不提供 |
| Jurisdictional restrictions | 提供 | 不提供 |
| 公开 bucket 域名 | Cloudflare 托管 | 不提供 |
| 平台备份 | 对象存储 PITR | 覆盖本机 SQLite 数据，不是对象存储 PITR |
