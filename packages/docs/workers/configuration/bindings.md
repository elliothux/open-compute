# Bindings

Use Wrangler's standard per-product fields. All names share the Worker `env` namespace.

```json
{
  "name": "app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "kv_namespaces": [{ "binding": "KV", "id": "<namespace-id>" }],
  "r2_buckets": [{ "binding": "BUCKET", "bucket_name": "files" }],
  "d1_databases": [{ "binding": "DB", "database_name": "app", "database_id": "<database-id>" }],
  "durable_objects": {
    "bindings": [{ "name": "COUNTER", "class_name": "Counter" }]
  },
  "queues": {
    "producers": [{ "binding": "JOBS", "queue": "jobs" }]
  },
  "workflows": [
    { "binding": "FLOW", "name": "flow", "class_name": "MyWorkflow" }
  ],
  "services": [
    { "binding": "AUTH", "service": "auth-worker", "entrypoint": "AuthEntrypoint" }
  ],
  "images": { "binding": "IMAGES" },
  "version_metadata": { "binding": "VERSION" }
}
```

Resource identifiers and names must resolve inside the same account. Wrangler handles standard provisioning where the declared command supports it; server validation rejects missing, cross-account, or unsupported bindings. Do not add the retired generic `bindings` object or its `type/id/permissions` records.

Regenerate local types after changing configuration:

```sh
bun run oc types --config wrangler.jsonc
```

Runtime detail: [Runtime APIs · bindings](/workers/runtime-apis/bindings).
