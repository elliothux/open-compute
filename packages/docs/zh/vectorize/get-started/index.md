# 上手

需要 `ocd` 就绪。先创建 Vectorize 索引，再在 `wrangler.jsonc` 中绑定。

## 1. 创建索引

对 live `ocd` 使用固定 Wrangler（或本地 Cloudflare v4 API）：

```sh
wrangler vectorize create embeddings --dimensions=768 --metric=cosine
```

## 2. 声明 binding

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

向量由你提供；open-compute 不负责生成 embedding。文档入库 + embedding + 检索见 [AI Search](/zh/ai-search/)。

## 4. 部署

```sh
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/vectorize/concepts/)。
