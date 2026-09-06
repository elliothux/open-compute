# AI Search

AI Search 对你上传的文件建索引，并支持关键词、向量或混合检索以及可选的 chat。Markdown Conversion 通过同一标准 `env.AI` binding 的 `toMarkdown()` / `supported()` 提供。

open-compute 使用 **operator 配置的 OpenAI-compatible provider** 实现上述表面。**不提供**完整 Workers AI 模型推理（`run()`、`models()`、AutoRAG 及其它无关推理）。

例如可用于：

- 上传文档并在 Worker 中检索
- 在生成回答前做混合检索
- 使用 `env.AI.toMarkdown()` 将 Office/PDF/HTML 转为 Markdown

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const result = await env.SEARCH.search({ query: "cache invalidation" });
    return Response.json(result);
  },
} satisfies ExportedHandler<{ SEARCH: AiSearchInstance }>;
```

绑定 namespace 和/或 instance；需要 Markdown Conversion 时再声明平台 `ai` binding：

```json
{
  "name": "search-app",
  "main": "src/index.ts",
  "ai_search_namespaces": [{ "binding": "SEARCH_NS", "namespace": "team" }],
  "ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
  "ai": { "binding": "AI" }
}
```

官方文档：[Cloudflare AI Search](https://developers.cloudflare.com/ai-search/)。绑定语法见[绑定](/zh/workers/configuration/bindings)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| AI Search Worker API | Namespace / instance / items / jobs / search / chat | 已声明表面相同 |
| Markdown Conversion | `env.AI.toMarkdown()` / `supported()` | 固定 overload 相同 |
| Embedding / chat 模型 | Cloudflare 托管 Workers AI | operator 固定的 OpenAI-compatible provider |
| 完整 Workers AI 推理 | `run()` / `models()` / AutoRAG | **不提供** |
| 对象字节 | 托管存储 | 选定的 Local 或 S3 authority |
| 就近存放 / 复制 | 全球 | 单机 |

## 本节

- [上手](/zh/ai-search/get-started/)
- [概念](/zh/ai-search/concepts/)
- [指南](/zh/ai-search/guides/)
- [示例](/zh/ai-search/examples/)
- [限制](/zh/ai-search/platform/limits)
- [行为差异](/zh/ai-search/platform/deviations)
