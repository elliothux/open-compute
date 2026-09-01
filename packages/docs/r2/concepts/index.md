# 概念

逻辑 bucket 是平台 catalog 里的资源。对象字节落在配置的 S3-compatible provider 上（`r2_prefix` 与平台 `prefix` 必须不相交）。Worker 通过 binding 看到 Cloudflare R2 那一套方法；同一份字节也可以用 provider 的 S3 API 读写——那是存储协议，不是 Cloudflare 管理面。

没有全球 placement：对象就在你配置的那一个 endpoint / bucket。

## 与 Cloudflare 相同

`put` / `get` / `head` / `delete` / `list`、range、条件（etag / 时间）、checksum、multipart、custom metadata、HTTP metadata。见 [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)。

## 故意不同

**`OC-R2-001`**：不声称 Cloudflare 全球 placement 或 replication。没有 Jurisdictional Restrictions 产品，没有 Cloudflare 托管的公开 bucket 域名。平台备份覆盖本机 SQLite 权威，不是对象存储 PITR。
