# Queues get started

Create a Queue through the supported Cloudflare v4 API:

```sh
curl -sS -X POST "$CLOUDFLARE_API_BASE_URL/accounts/$CLOUDFLARE_ACCOUNT_ID/queues" \
  -H "Authorization: Bearer $CLOUDFLARE_API_TOKEN" \
  -H "content-type: application/json" \
  -d '{"queue_name":"jobs"}'
```

Use standard Wrangler producer and consumer configuration:

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

The producer uses `env.QUEUE.send`; the Worker exports the standard `queue` handler.

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/queues/concepts/).
