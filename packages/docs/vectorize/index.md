# Vectorize

Vectorize is a vector index binding for Workers. Store embeddings you already computed, filter by metadata, and run similarity queries. open-compute implements the stable post-beta [`Vectorize`](https://developers.cloudflare.com/vectorize/) API with deterministic **exact** search on one node.

For example, you can use Vectorize for:

- Semantic search over document embeddings
- Recommendation / nearest-neighbor lookups from a Worker
- Metadata-filtered retrieval before an LLM call

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { matches } = await env.VECTORIZE.query([0.12, 0.34, /* … */], {
      topK: 5,
      returnMetadata: "indexed",
    });
    return Response.json(matches);
  },
} satisfies ExportedHandler<{ VECTORIZE: Vectorize }>;
```

Create an index (Wrangler or v4), then bind it:

```json
{
  "name": "vector-app",
  "main": "src/index.ts",
  "vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
}
```

Official reference: [Cloudflare Vectorize](https://developers.cloudflare.com/vectorize/). Binding grammar: [bindings](/workers/configuration/bindings).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | Stable post-beta `Vectorize` (`describe` / `query` / `queryById` / `insert` / `upsert` / `deleteByIds` / `getByIds`) | Same methods and response shape |
| Search | Managed approximate / distributed index | Deterministic **exact** search on one node |
| Dimensions / metrics | 32–1536; cosine, euclidean, dot-product | Same public ranges and score/order semantics |
| Mutations | Asynchronous `mutationId` | Durable async mutations on local authority |
| Beta `VectorizeIndex` | Legacy | Out of scope — not provided |
| Placement / replication | Global | Single-node; Local/S3 as configured by the operator |

## Next

- [Get started](/vectorize/get-started/)
- [Concepts](/vectorize/concepts/)
- [Guides](/vectorize/guides/)
- [Examples](/vectorize/examples/)
- [Limits](/vectorize/platform/limits)
- [Behavior differences](/vectorize/platform/deviations)
