# Concepts

Each public index is one local SQLite authority. Mutations (`insert` / `upsert` / `deleteByIds`) are durable and asynchronous: the Worker receives a `mutationId`; applied visibility advances through the local coordinator. Queries (`query` / `queryById` / `getByIds` / `describe`) read the applied frontier.

Search is **exact** (full scan with metadata pre-filter), not Cloudflare's distributed approximate topology. Scores follow the index metric: cosine, euclidean, or dot-product.

Metadata filters use the indexed metadata surface (`$eq`, `$ne`, `$in`, `$nin`, `$lt`, `$lte`, `$gt`, `$gte`, and combinations). Create metadata indexes before filtering on a property, matching Cloudflare's contract.

Not provided:

- Deprecated beta `VectorizeIndex` class
- Hosted global placement / replication
- Cloudflare dashboard billing or fleet-scale quotas (for example 10M–20M vectors/index as a local promise)
- Automatic embedding generation (use your model or [AI Search](/ai-search/))

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Index authority | Managed Vectorize service | Per-index local SQLite |
| Query algorithm | Approximate / distributed | Exact / local |
| Async mutations | `mutationId` | Same public shape; local durable coordinator |
| Beta `VectorizeIndex` | Legacy | Not provided |
