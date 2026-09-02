# Queues 上手

通过受支持的 Cloudflare v4 API 创建 Queue：

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/queues" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"queue_name":"jobs"}'
```

使用标准 Wrangler producer/consumer 配置：

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "queues": {
    "producers": [{ "binding": "QUEUE", "queue": "jobs" }],
    "consumers": [{ "queue": "jobs", "max_batch_size": 10, "max_batch_timeout": 5 }]
  }
}
```

producer 使用 `env.QUEUE.send`；Worker 导出标准 `queue` handler。

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/queues/concepts/)。
