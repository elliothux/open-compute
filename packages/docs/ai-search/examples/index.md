# Examples

Upload a document, then hybrid-search and convert Markdown with `env.AI`.

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

Setup: [Get started](/ai-search/get-started/). Options: [Guides](/ai-search/guides/).
