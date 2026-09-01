# Concepts

A logical bucket is a catalog resource. Object bytes live on the configured S3-compatible provider (`r2_prefix` must be disjoint from the platform `prefix`). The Worker sees Cloudflare R2 methods through the binding. The same bytes can also be read and written with the provider's S3 API — that is the storage protocol, not the Cloudflare control plane.

There is no global placement: objects sit at the one endpoint / bucket you configured.

## Same as Cloudflare

`put` / `get` / `head` / `delete` / `list`, range, conditionals (etag / time), checksums, multipart, custom metadata, HTTP metadata. See the [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/).

## Intentional differences

**`OC-R2-001`**: no Cloudflare global placement or replication. No Jurisdictional Restrictions product, no Cloudflare-hosted public bucket hostname. Platform backups cover local SQLite authority, not object-store PITR.
