# AI Search

AI Search indexes files you upload, then runs keyword, vector, or hybrid retrieval and optional chat. Markdown Conversion is exposed on the same standard `env.AI` binding via `toMarkdown()` / `supported()`.

open-compute implements these surfaces with **operator-configured OpenAI-compatible providers**. Full Workers AI model inference (`run()`, `models()`, AutoRAG, and unrelated inference) is **not** provided.

For example, you can use AI Search for:

- Uploading documents and searching them from a Worker
- Hybrid retrieval before generating an answer
- Converting Office/PDF/HTML inputs to Markdown with `env.AI.toMarkdown()`

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const result = await env.SEARCH.search({ query: "cache invalidation" });
    return Response.json(result);
  },
} satisfies ExportedHandler<{ SEARCH: AiSearchInstance }>;
```

Bind a namespace and/or instance, plus the platform `ai` binding when you need Markdown Conversion:

```json
{
  "name": "search-app",
  "main": "src/index.ts",
  "ai_search_namespaces": [{ "binding": "SEARCH_NS", "namespace": "team" }],
  "ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
  "ai": { "binding": "AI" }
}
```

Official reference: [Cloudflare AI Search](https://developers.cloudflare.com/ai-search/). Binding grammar: [bindings](/workers/configuration/bindings).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| AI Search Worker API | Namespace / instance / items / jobs / search / chat | Same declared surface |
| Markdown Conversion | `env.AI.toMarkdown()` / `supported()` | Same pinned overloads |
| Embeddings / chat models | Cloudflare-hosted Workers AI | Operator-pinned OpenAI-compatible providers |
| Full Workers AI inference | `run()` / `models()` / AutoRAG | **Not provided** |
| Object bytes | Hosted storage | Selected Local or S3 authority |
| Placement / replication | Global | Single-node |

## Next

- [Get started](/ai-search/get-started/)
- [Concepts](/ai-search/concepts/)
- [Guides](/ai-search/guides/)
- [Examples](/ai-search/examples/)
- [Limits](/ai-search/platform/limits)
- [Behavior differences](/ai-search/platform/deviations)
