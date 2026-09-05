# 指南

## Binding

```json
"ai_search_namespaces": [{ "binding": "SEARCH_NS", "namespace": "team" }],
"ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
"ai": { "binding": "AI" }
```

Namespace binding 可在该 namespace 内创建/列出 instance。Instance binding 只能访问固定 instance。

## Items 与 jobs

```ts
await env.SEARCH.items.upload("notes.md", blob);
await env.SEARCH.items.list();
await env.SEARCH.jobs.list();
```

## 检索与 chat

```ts
await env.SEARCH.search({
  query: "durable objects alarms",
  ai_search_options: { retrieval: { max_num_results: 10 } },
});

await env.SEARCH.chatCompletions({
  messages: [{ role: "user", content: "Summarize cache behavior" }],
  stream: true,
});
```

多 instance 的 search/chat 通过 namespace binding，并传入 `ai_search_options.instance_ids`。

## Markdown Conversion

```ts
await env.AI.toMarkdown(
  { name: "spec.pdf", blob },
  { conversionOptions: { output: { format: "markdown" }, pdf: { metadata: true } } },
);
```

已接纳格式包括文本、Markdown、HTML、XML、JSON、CSV、文本层 PDF，以及常见 Office/ODF 表格与文档。图像/OCR 与不支持的 Office 变体会失败关闭。

官方文档：[AI Search](https://developers.cloudflare.com/ai-search/)。
