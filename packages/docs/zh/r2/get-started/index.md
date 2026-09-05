# R2 上手

通过受支持的 Cloudflare v4 API 创建逻辑 bucket：

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/r2/buckets" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"name":"my-bucket"}'
```

用标准 Wrangler 配置绑定 bucket 名称：

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "r2_buckets": [{ "binding": "BUCKET", "bucket_name": "my-bucket" }]
}
```

Worker 使用标准 `R2Bucket` API。bucket/object 管理走 `/client/v4`，Worker 流量走 binding。operator 选定的 Local/S3 backend 是内部实现，不改变这些 API。

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/r2/concepts/)和[指南](/zh/r2/guides/)。
