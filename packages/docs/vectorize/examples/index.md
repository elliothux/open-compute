# Examples

Upsert vectors, then query nearest neighbors with a metadata filter.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/upsert" && request.method === "POST") {
      const vectors = await request.json<VectorizeVector[]>();
      return Response.json(await env.VECTORIZE.upsert(vectors));
    }
    if (url.pathname === "/query" && request.method === "POST") {
      const vector = await request.json<number[]>();
      const { matches } = await env.VECTORIZE.query(vector, {
        topK: 5,
        filter: { published: true },
        returnMetadata: "all",
      });
      return Response.json(matches);
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<{ VECTORIZE: Vectorize }>;
```

```json
{
  "name": "vector-app",
  "main": "src/index.ts",
  "vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
}
```

Setup: [Get started](/vectorize/get-started/). Options: [Guides](/vectorize/guides/).
