# Get started

`ocd` must be ready. The operator must configure OpenAI-compatible embedding (and optional rewrite / rerank / chat) providers in the platform config before AI Search can index or answer.

## 1. Create namespace and instance

Use pinned Wrangler against live `ocd` (commands follow Cloudflare AI Search / Wrangler resource flow).

## 2. Declare bindings

```json
{
  "name": "search-app",
  "main": "src/index.ts",
  "ai_search_namespaces": [{ "binding": "SEARCH_NS", "namespace": "team" }],
  "ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
  "ai": { "binding": "AI" }
}
```

```sh
bun run oc types --config wrangler.jsonc
```

## 3. Upload and search

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === "PUT") {
      const file = await request.blob();
      const item = await env.SEARCH.items.upload("guide.pdf", file);
      return Response.json(item);
    }
    return Response.json(await env.SEARCH.search({ query: "how does cache work?" }));
  },
} satisfies ExportedHandler<Env>;
```

Upload returns quickly with a queued item; parse / chunk / embed / index continue asynchronously on the node.

## 4. Markdown Conversion

```ts
const formats = await env.AI.supported();
const md = await env.AI.toMarkdown({ name: "page.html", blob: htmlBlob });
```

`env.AI.run()`, `models()`, gateway, AutoRAG, and other inference members are rejected.

## 5. Deploy

```sh
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/ai-search/concepts/).
