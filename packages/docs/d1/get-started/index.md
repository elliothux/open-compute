# D1 get started

Create a database through the supported Cloudflare v4 API:

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/d1/database" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"name":"my-db"}'
```

Bind the returned UUID with standard Wrangler configuration:

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "d1_databases": [
    { "binding": "DB", "database_name": "my-db", "database_id": "<database-uuid>" }
  ]
}
```

Use standard D1 Worker APIs and Wrangler migration commands. Generate local types and deploy:

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/d1/concepts/) and [Guides](/d1/guides/).
