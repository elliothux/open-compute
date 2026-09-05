# Concepts

AI Search separates:

- **Namespace** — catalog of instances (parent resource; no per-namespace database)
- **Instance** — one SQLite authority plus immutable source objects on the configured Local or S3 backend
- **`env.AI`** — platform binding for Markdown Conversion only (`toMarkdown` / `supported` / `aiGatewayLogId`)

Indexing pipeline: upload → parse (document parser child) → chunk → embed (operator provider) → FTS + vector generations. Search modes: keyword, vector, and hybrid. Chat uses retrieval plus an optional generation provider; SSE streaming is supported when configured.

Providers are pinned by the operator (model revision, dimensions, tokenizer digest, timeouts). open-compute does not ship embedded model weights or claim Cloudflare-hosted model availability.

Not provided:

- Full Workers AI inference (`run`, `models`, AutoRAG, unrelated `env.AI` members)
- Cloudflare-hosted automatic provider selection, billing, dashboard connectors
- Global index placement / replication

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Data path | Hosted AI Search | Per-instance SQLite + Local/S3 objects |
| Models | Workers AI | Operator OpenAI-compatible endpoints |
| Markdown Conversion | `env.AI.toMarkdown` | Same subset |
| Full Workers AI | Available | Not provided |
