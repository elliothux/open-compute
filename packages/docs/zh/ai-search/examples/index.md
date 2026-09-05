# 示例

上传文档后做混合检索，并用 `env.AI` 做 Markdown 转换。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/upload" && request.method === "PUT") {
      const name = url.searchParams.get("name") ?? "upload.bin";
      return Response.json(await env.SEARCH.items.upload(name, await request.blob()));
    }
    if (url.pathname === "/search") {
      const q = url.searchParams.get("q") ?? "";
      return Response.json(await env.SEARCH.search({ query: q }));
    }
    if (url.pathname === "/markdown" && request.method === "POST") {
      return Response.json(
        await env.AI.toMarkdown({ name: "body.html", blob: await request.blob() }),
      );
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<{ SEARCH: AiSearchInstance; AI: Ai }>;
```

```json
{
  "name": "search-app",
  "main": "src/index.ts",
  "ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
  "ai": { "binding": "AI" }
}
```

上手见[上手](/zh/ai-search/get-started/)。选项见[指南](/zh/ai-search/guides/)。
