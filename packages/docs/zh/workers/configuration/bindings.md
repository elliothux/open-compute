# 绑定

使用 Wrangler 的标准分产品字段；所有名称共享 Worker `env` namespace。

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

资源 ID 和名称必须解析到同一 account。声明的标准命令支持 provisioning 时由 Wrangler 负责；server validation 会拒绝缺失、跨 account 或不支持的 binding。不要再添加已删除的通用 `bindings` 对象或 `type/id/permissions` 记录。

配置变更后重新生成本地类型：

```sh
bun run oc types --config wrangler.jsonc
```

运行时细节：[Runtime APIs · bindings](/zh/workers/runtime-apis/bindings)。
