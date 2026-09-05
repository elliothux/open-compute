# Behavior differences

The R2 Worker API matches Cloudflare, including exact single/part/multipart ETag semantics and lowercase-hex `ssecKeyMd5`. Object bytes live on the configured Local or S3 authority on the single node.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `head` / `get` / `put` / `delete` / `list`, conditional writes, checksums, multipart, HTTP metadata |
| Object bytes | Cloudflare R2 storage | Configured Local or S3 authority on one node |
| Global placement | Available | Not provided |
| r2.dev public product | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| REST / `/client/v4` | Available | Compatible bucket and object operations |

See [Compatibility](/platform/compatibility).
