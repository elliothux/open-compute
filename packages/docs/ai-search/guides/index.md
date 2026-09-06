# Guides

## Bindings

```json
"ai_search_namespaces": [{ "binding": "SEARCH_NS", "namespace": "team" }],
"ai_search": [{ "binding": "SEARCH", "instance_name": "docs" }],
"ai": { "binding": "AI" }
```

Namespace bindings can create/list instances in that namespace. Instance bindings talk to one fixed instance.

## Items and jobs

```ts
await env.SEARCH.items.upload("notes.md", blob);
await env.SEARCH.items.list();
await env.SEARCH.jobs.list();
```

## Search and chat

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

Multi-instance search/chat goes through the namespace binding with `ai_search_options.instance_ids`.

## Markdown Conversion

```ts
await env.AI.toMarkdown(
  { name: "spec.pdf", blob },
  { conversionOptions: { output: { format: "markdown" }, pdf: { metadata: true } } },
);
```

Admitted formats include text, Markdown, HTML, XML, JSON, CSV, text-layer PDF, and common Office/ODF spreadsheets/documents. Images/OCR and unsupported Office variants fail closed.

Official docs: [AI Search](https://developers.cloudflare.com/ai-search/).
