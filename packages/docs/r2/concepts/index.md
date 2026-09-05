# Concepts

A logical bucket is a catalog resource. Object bytes live on the configured Local or S3 object authority (`r2_prefix` must be disjoint from the platform `prefix`). The Worker sees Cloudflare R2 methods through the binding. When S3 is selected, its provider API is an operator storage path, not the Cloudflare control plane; Local exposes no filesystem or S3 endpoint to tenants.

Global placement is not provided. Objects sit on the one local machine or at the one S3 endpoint/bucket configured for the platform.

`put` / `get` / `head` / `delete` / `list`, range, conditionals (etag / time), checksums, multipart, custom metadata, and HTTP metadata match the [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/). Single-part and part ETags are lowercase MD5; completed multipart ETags use Cloudflare's ordered part-MD5 formula. `ssecKeyMd5` is lowercase hexadecimal.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `put` / `get` / `head` / `delete` / `list`, range, conditionals, checksums, multipart, metadata |
| Object bytes | Cloudflare R2 storage | Configured Local or S3 authority on one node |
| Global placement | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| Public bucket hostname | Cloudflare-hosted | Not provided |
| Platform backups | Object-store PITR | Cover local SQLite authority, not object-store PITR |
