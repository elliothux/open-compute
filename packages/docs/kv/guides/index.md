# Guides

## Create a namespace

See [Get started](/kv/get-started/). Namespace CRUD uses `/client/v4/accounts/{account_id}/storage/kv/namespaces` through the official SDK or pinned Wrangler.

## Bind

```json
"kv_namespaces": [{ "binding": "KV", "id": "<namespace-id>" }]
```

After edits, run `bun run oc types --config wrangler.jsonc`.

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

Bulk get: `env.KV.get(["a", "b"])`. Do not inflate stream values through JSON; the cap is still 25 MiB. Full methods: [Cloudflare KV API](https://developers.cloudflare.com/kv/api/).

A write is immediately visible on this node. There is no global propagation wait.
