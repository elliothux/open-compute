# Get started

`ocd` must be ready. Create a Vectorize index, then bind it from `wrangler.jsonc`.

## 1. Create an index

Use pinned Wrangler against live `ocd` (or the local Cloudflare v4 API):

```sh
wrangler vectorize create embeddings --dimensions=768 --metric=cosine
```

## 2. Declare the binding

```json
{
  "name": "vector-app",
  "main": "src/index.ts",
  "vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
}
```

```sh
bun run oc types --config wrangler.jsonc
```

## 3. Worker

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === "PUT") {
      const body = await request.json<{ id: string; values: number[] }>();
      const { mutationId } = await env.VECTORIZE.upsert([
        { id: body.id, values: body.values, metadata: { source: "api" } },
      ]);
      return Response.json({ mutationId });
    }
    const { matches } = await env.VECTORIZE.query(
      await request.json<number[]>(),
      { topK: 10, returnMetadata: "all" },
    );
    return Response.json(matches);
  },
} satisfies ExportedHandler<Env>;
```

You supply vectors; open-compute does not generate embeddings. For document ingest + embedding + retrieval, see [AI Search](/ai-search/).

## 4. Deploy

```sh
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/vectorize/concepts/).
