# Concepts

A logical bucket is a catalog resource. Object bytes live on the configured S3-compatible provider (`r2_prefix` must be disjoint from the platform `prefix`). The Worker sees Cloudflare R2 methods through the binding. The same bytes can also be read and written with the provider's S3 API — that is the storage protocol, not the Cloudflare control plane.

Global placement is not provided. Objects sit at the one endpoint / bucket you configured.

`put` / `get` / `head` / `delete` / `list`, range, conditionals (etag / time), checksums, multipart, custom metadata, and HTTP metadata match the [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `put` / `get` / `head` / `delete` / `list`, range, conditionals, checksums, multipart, metadata |
| Object bytes | Cloudflare R2 storage | Configured S3-compatible provider |
| Global placement | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| Public bucket hostname | Cloudflare-hosted | Not provided |
| Platform backups | Object-store PITR | Cover local SQLite authority, not object-store PITR |
