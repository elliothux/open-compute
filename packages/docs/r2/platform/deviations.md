# Behavior differences

The R2 Worker API matches Cloudflare. Object bytes live on the configured S3-compatible provider.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `head` / `get` / `put` / `delete` / `list`, conditional writes, checksums, multipart, HTTP metadata |
| Object bytes | Cloudflare R2 storage | Configured S3-compatible provider |
| Global placement | Available | Not provided |
| r2.dev public product | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| REST / `client.v4` | Available | Not provided; use the Worker binding or the provider S3 API |

See [Compatibility](/platform/compatibility).
