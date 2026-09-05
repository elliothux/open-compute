# Limits

Worker API ceilings follow Cloudflare Vectorize where declared: dimensions 32–1536 (float32), vector id and namespace ≤ 64 UTF-8 bytes, metadata ≤ 10 KiB/vector, ≤ 10 metadata indexes per index, mutation batch ≤ 1000, `topK` ≤ 100 (≤ 50 with values or all metadata).

Local resource quotas are operator-owned and frozen at index create time. Embedded defaults include on the order of **100k vectors/index** (schema ceiling lower than Cloudflare's hosted multi-million scale) plus a logical bytes quota and bounded CPU concurrency. Live numbers:

```sh
ocd capabilities --json
```

These are not Cloudflare plan quotas. Approximate ANN capacity (hosted 10M+/index class) is not provided.
