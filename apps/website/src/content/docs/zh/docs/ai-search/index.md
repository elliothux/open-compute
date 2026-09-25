---
title: "AI Search"
---

AI Search 对你上传的文件建索引，并支持关键词、向量或混合检索以及可选的 chat。Markdown Conversion 通过同一标准 `env.AI` binding 的 `toMarkdown()` / `supported()` 提供。

open-compute 使用 **operator 配置的 OpenAI-compatible provider** 实现上述表面。**不提供**完整 Workers AI 模型推理（`run()`、`models()`、AutoRAG 及其它无关推理）。

例如可用于：

- 上传文档或索引有界 R2 source，再从 Worker 中检索
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

## 手动外部 source（open-compute 扩展）

`open-compute:manual` 是给已拥有 immutable 文件 revision 的应用使用的 namespaced API superset。它不扫描 source，也不持久化第二份完整源对象，只保存 locator 与正常的 parse/index 派生状态。每个 source authority 配置一个 loopback provider：

```toml
[ai.source_providers.files]
endpoint = "http://127.0.0.1:9080/provider"
source = "files"
credential = { env = "FILES_SOURCE_TOKEN" }
max_source_bytes = 67108864
```

provider 对 exact `{ source, key, revision }` 请求实现带认证的 `POST <endpoint>/resolve` 与 `POST <endpoint>/read`。`resolve` 返回 `revision`、`contentType`、`size`、`sha256`；`read` 通过 `Content-Type`、`X-Open-Compute-Revision`、`X-Open-Compute-Size`、`X-Open-Compute-Sha256` 重复这些事实。redirect、压缩、revision 漂移、digest 不符或超限正文均 fail closed。

只使用 `open-compute:ai-search` 导出的扩展类型：

```ts
import type { OpenComputeAiSearchNamespace } from "open-compute:ai-search";

const index = await env.SEARCH_NS.openComputeCreateManual("files", {
  id: "documents",
  embedding_model: "company/qwen-embedding",
  index_method: { keyword: true, vector: true },
});
await index.items.openComputeUpsert({
  key: "files/blob-123",
  revision: "immutable-revision",
  contentType: "application/pdf",
  metadata: { team_id: "team-1" },
  waitForCompletion: false,
});
```

重复同一 exact revision 是幂等操作。revision 改变时复用既有 generation fence；delete 只删除 locator 与派生索引状态。官方 Cloudflare 管理表面看不到 manual instance 和扩展字段。

对于 manual instance，item info、list、download 与 search 结果都会返回 `open_compute_source`，包含 provider、source namespace、key 与 exact revision，供应用重新授权原始对象。

官方文档：[Cloudflare AI Search](https://developers.cloudflare.com/ai-search/)。绑定语法见[绑定](/zh/docs/workers/configuration/bindings/)。

## 兼容性

| 主题                  | Cloudflare                                          | open-compute                               |
| --------------------- | --------------------------------------------------- | ------------------------------------------ |
| AI Search Worker API  | Namespace / instance / items / jobs / search / chat | 已声明表面相同                             |
| Markdown Conversion   | `env.AI.toMarkdown()` / `supported()`               | 固定 overload 相同                         |
| Embedding / chat 模型 | Cloudflare 托管 Workers AI                          | operator 固定的 OpenAI-compatible provider |
| 完整 Workers AI 推理  | `run()` / `models()` / AutoRAG                      | **不提供**                                 |
| 对象字节              | 托管存储                                            | 选定的 Local 或 S3 authority               |
| 就近存放 / 复制       | 全球                                                | 单机                                       |
| 手动外部 source       | 非官方 member                                       | `open-compute:manual` namespaced 扩展      |

下一步：[使用 bindings 开发](/zh/docs/develop/) · [兼容性与限制](/zh/docs/reference/)
