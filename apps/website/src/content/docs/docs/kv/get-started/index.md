---
title: "KV get started"
---

Create a namespace through the supported Cloudflare v4 API:

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/storage/kv/namespaces" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"title":"my-kv"}'
```

Bind the returned namespace ID with standard Wrangler configuration:

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "kv_namespaces": [{ "binding": "KV", "id": "<namespace-id>" }]
}
```

The Worker uses the standard `KVNamespace` methods such as `get`, `put`, `delete`, and `list`. Generate local types and deploy through pinned Wrangler:

```sh
bun run oc types --config wrangler.jsonc
ocd wrangler deploy --config wrangler.jsonc
```

Next: [Concepts](/docs/kv/concepts/) and [Guides](/docs/kv/guides/).
