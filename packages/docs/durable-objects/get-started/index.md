# Durable Objects get started

Durable Object namespaces are owned by Worker exports and standard migrations; there is no manual namespace-create transport.

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "durable_objects": {
    "bindings": [{ "name": "COUNTER", "class_name": "Counter" }]
  },
  "migrations": [
    { "tag": "v1", "new_sqlite_classes": ["Counter"] }
  ]
}
```

Export the `Counter` class, use `env.COUNTER.idFromName` and `get` in Worker code, then deploy through pinned Wrangler:

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

The Dashboard vendor extension provides read-only namespace and object inventory; lifecycle remains declarative through Worker versions and migrations.

Next: [Concepts](/durable-objects/concepts/).
