# 指南

## 创建 namespace

见[上手](/kv/get-started/)。本平台 `POST /v1/accounts/{accountId}/kv/namespaces`，body `{ "name": "..." }`，需要 `idempotency-key`。不提供 Cloudflare REST。

## 绑定

```json
"bindings": {
  "KV": { "type": "kv_namespace", "id": "<resourceId>" }
}
```

可选 `permissions`。改完后运行 `bun run oc types --config open-compute.json`。

## get / put / list / delete

```ts
await env.KV.put("user:1", JSON.stringify({ plan: "pro" }), {
  expirationTtl: 3600,
  metadata: { source: "signup" },
});
const text = await env.KV.get("user:1");
const json = await env.KV.get("user:1", "json");
const { value, metadata } = await env.KV.getWithMetadata("user:1", "json");
const listed = await env.KV.list({ prefix: "user:", limit: 100 });
await env.KV.delete("user:1");
```

bulk get：`env.KV.get(["a", "b"])`。stream 值不要经 JSON 膨胀；上限仍是 25 MiB。完整方法见 [Cloudflare KV API](https://developers.cloudflare.com/kv/api/)。

写入后立即在本机可见。无全球传播等待。
