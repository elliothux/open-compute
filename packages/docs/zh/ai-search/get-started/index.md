# 上手

需要 `ocd` 就绪。在 AI Search 能够建索引或回答之前，operator 须在平台配置中写好 OpenAI-compatible 的 embedding（以及可选的 rewrite / rerank / chat）provider。

## 1. 创建 namespace 与 instance

对 live `ocd` 使用固定 Wrangler（命令遵循 Cloudflare AI Search / Wrangler 资源流程）。

## 2. 声明 binding

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

## 3. 上传与检索

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

上传快速返回 queued item；parse / chunk / embed / index 在本机异步继续。

## 4. Markdown Conversion

```ts
const formats = await env.AI.supported();
const md = await env.AI.toMarkdown({ name: "page.html", blob: htmlBlob });
```

`env.AI.run()`、`models()`、gateway、AutoRAG 及其它推理成员会被拒绝。

## 5. 部署

```sh
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/ai-search/concepts/)。
