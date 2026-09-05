# 指南

## 绑定索引

```json
"vectorize": [{ "binding": "VECTORIZE", "index_name": "embeddings" }]
```

`index_name` 必须解析为同一账号内已存在的索引。

## insert 与 upsert

```ts
await env.VECTORIZE.insert([
  { id: "doc-1", values: embedding, namespace: "docs", metadata: { lang: "en" } },
]);
await env.VECTORIZE.upsert([
  { id: "doc-1", values: embedding, metadata: { lang: "en", rev: 2 } },
]);
```

`insert` 不覆盖已有 id。`upsert` 替换完整向量记录。批次上限见[限制](/zh/vectorize/platform/limits)。

## 带 metadata 的查询

```ts
const { matches } = await env.VECTORIZE.query(embedding, {
  topK: 20,
  namespace: "docs",
  filter: { lang: "en", year: { $gte: 2024 } },
  returnValues: false,
  returnMetadata: "indexed",
});
```

`returnMetadata`：`"none"` | `"indexed"` | `"all"`（布尔 `true` 归一为 `"all"`）。

## queryById / getByIds / describe

```ts
await env.VECTORIZE.queryById("doc-1", { topK: 5 });
await env.VECTORIZE.getByIds(["doc-1", "doc-2"]);
const info = await env.VECTORIZE.describe();
```

官方 API：[Vectorize Workers API](https://developers.cloudflare.com/vectorize/reference/client-api/)。
