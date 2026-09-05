# R2 get started

Create a logical bucket through the supported Cloudflare v4 API:

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"name":"my-bucket"}'
```

Bind the bucket name with standard Wrangler configuration:

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "r2_buckets": [{ "binding": "BUCKET", "bucket_name": "my-bucket" }]
}
```

Worker code uses standard `R2Bucket` methods. Bucket and object management use `/client/v4`; Worker traffic uses the binding. The operator-selected Local/S3 backend is internal and does not change this API.

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/r2/concepts/) and [Guides](/r2/guides/).
