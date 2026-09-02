# Durable Objects 上手

Durable Object namespace 由 Worker export 和标准 migration 管理；不存在手工 namespace-create transport。

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

导出 `Counter` class，在 Worker 中使用 `env.COUNTER.idFromName` 和 `get`，再通过固定 Wrangler 部署：

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

Dashboard vendor extension 只提供 namespace/object inventory；lifecycle 仍通过 Worker version 与 migration 声明管理。

下一步：[概念](/zh/durable-objects/concepts/)。
