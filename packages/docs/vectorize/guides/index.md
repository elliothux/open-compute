# Guides

## Bind an index

```json
"vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
```

`index_name` must resolve to an existing index in the same account.

## Insert and upsert

```ts
await env.VECTORIZE.insert([
  { id: "doc-1", values: embedding, namespace: "docs", metadata: { lang: "en" } },
]);
await env.VECTORIZE.upsert([
  { id: "doc-1", values: embedding, metadata: { lang: "en", rev: 2 } },
]);
```

`insert` does not overwrite an existing id. `upsert` replaces the full vector record. Batches are capped (see [Limits](/vectorize/platform/limits)).

## Query with metadata

```ts
const { matches } = await env.VECTORIZE.query(embedding, {
  topK: 20,
  namespace: "docs",
  filter: { lang: "en", year: { $gte: 2024 } },
  returnValues: false,
  returnMetadata: "indexed",
});
```

`returnMetadata`: `"none"` | `"indexed"` | `"all"` (boolean `true` normalizes to `"all"`).

## queryById / getByIds / describe

```ts
await env.VECTORIZE.queryById("doc-1", { topK: 5 });
await env.VECTORIZE.getByIds(["doc-1", "doc-2"]);
const info = await env.VECTORIZE.describe();
```

Official API: [Vectorize Workers API](https://developers.cloudflare.com/vectorize/reference/client-api/).
