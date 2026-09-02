# KV 上手

通过受支持的 Cloudflare v4 API 创建 namespace：

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/storage/kv/namespaces" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"title":"my-kv"}'
```

用标准 Wrangler 配置绑定返回的 namespace ID：

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "kv_namespaces": [{ "binding": "KV", "id": "<namespace-id>" }]
}
```

Worker 使用标准 `KVNamespace` 的 `get`、`put`、`delete`、`list`。生成本地类型并通过固定 Wrangler 部署：

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/kv/concepts/)和[指南](/zh/kv/guides/)。
