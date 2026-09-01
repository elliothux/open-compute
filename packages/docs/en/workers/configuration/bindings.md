# Bindings

Resources on `env`. Declare them with the `bindings` object, the `services` array, and `assets` / `images` / `version_metadata`.

```json
{
  "name": "app",
  "main": "src/index.ts",
  "vars": { "LOG_LEVEL": "info" },
  "secrets": { "TOKEN": { "env": "MY_TOKEN" } },
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-id>" },
    "BUCKET": { "type": "r2_bucket", "id": "<r2-id>" },
    "DB": { "type": "d1_database", "id": "<d1-id>" },
    "COUNTER": { "type": "do_namespace", "id": "<do-id>", "className": "Counter" },
    "JOBS": { "type": "queue_producer", "id": "<queue-id>" },
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  },
  "services": [
    { "binding": "AUTH", "service": "auth-worker", "entrypoint": "AuthEntrypoint" }
  ],
  "images": { "binding": "IMAGES" },
  "version_metadata": { "binding": "VERSION", "tag": "release-1" }
}
```

`permissions` is optional `{ "read": true, "write": false }`. Durable Object and Workflow bindings must provide `className`: it is only used to check class semantics in generated framework config, and is not sent to the platform as a resource ID. Workflows may set a `schedules` string array.

Regenerate types after changing config:

```sh
bun run oc types --config open-compute.json
```

Runtime detail: [Runtime APIs · bindings](/en/workers/runtime-apis/bindings).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Tenants only see declared names | Yes | Yes |
| Worker-side APIs for KV / R2 / D1 / DO / Queue / Workflow / Service / Assets / Images / Version Metadata | Yes — [Cloudflare bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | Same symbols |
| Config shape | Wrangler per-product top-level arrays (`kv_namespaces`, …) | `bindings` object with `{type, id, permissions?}` |
| Service Bindings | Same-account Workers | `services: [{binding, service, entrypoint?}]`; deploy time resolves the same-account Worker name and freezes a target ID; same-platform only |
| Workers AI, Vectorize, Hyperdrive, mTLS, Rate Limiting, Secrets Store, Analytics Engine, Browser Rendering | Yes | Not provided |
| Writing config creates the resource | Wrangler can create some resources | The resource must already exist on the platform |

