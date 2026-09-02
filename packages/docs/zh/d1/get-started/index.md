# D1 上手

通过受支持的 Cloudflare v4 API 创建 database：

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/d1/database" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"name":"my-db"}'
```

用标准 Wrangler 配置绑定返回的 UUID：

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

Worker 使用标准 D1 API，migration 使用 Wrangler 标准命令。生成本地类型并部署：

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/d1/concepts/)和[指南](/zh/d1/guides/)。
