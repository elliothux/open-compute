# Vectorize

Vectorize 是 Workers 的向量索引 binding。写入你已计算好的 embedding，按 metadata 过滤，并做相似度查询。open-compute 实现稳定后 beta 的 [`Vectorize`](https://developers.cloudflare.com/vectorize/) API，在单机上提供确定性的**精确**检索。

例如可用于：

- 文档 embedding 的语义检索
- 从 Worker 发起推荐 / 近邻查询
- 在调用 LLM 前做带 metadata 过滤的检索

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { matches } = await env.VECTORIZE.query([0.12, 0.34, /* … */], {
      topK: 5,
      returnMetadata: "indexed",
    });
    return Response.json(matches);
  },
} satisfies ExportedHandler<{ VECTORIZE: Vectorize }>;
```

创建索引（Wrangler 或 v4）后绑定：

```json
{
  "name": "vector-app",
  "main": "src/index.ts",
  "vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
}
```

官方文档：[Cloudflare Vectorize](https://developers.cloudflare.com/vectorize/)。绑定语法见[绑定](/zh/workers/configuration/bindings)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | 稳定后 beta 的 `Vectorize`（`describe` / `query` / `queryById` / `insert` / `upsert` / `deleteByIds` / `getByIds`） | 方法与响应形状相同 |
| 检索 | 托管近似 / 分布式索引 | 单机确定性**精确**检索 |
| 维度 / 度量 | 32–1536；cosine、euclidean、dot-product | 公开范围与 score/order 语义相同 |
| Mutation | 异步 `mutationId` | 本地 authority 上的持久异步 mutation |
| Beta `VectorizeIndex` | 遗留 | 不在范围 — 不提供 |
| 就近存放 / 复制 | 全球 | 单机；按 operator 配置使用 Local/S3 |

## 本节

- [上手](/zh/vectorize/get-started/)
- [概念](/zh/vectorize/concepts/)
- [指南](/zh/vectorize/guides/)
- [示例](/zh/vectorize/examples/)
- [限制](/zh/vectorize/platform/limits)
- [行为差异](/zh/vectorize/platform/deviations)
