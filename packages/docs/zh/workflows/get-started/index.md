# Workflows 上手

Wrangler 部署导出 class 的 Worker 时，通过官方 API 创建或更新 Workflow definition。使用标准配置绑定：

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "workflows": [
    { "binding": "FLOW", "name": "orders", "class_name": "MyWorkflow" }
  ]
}
```

导出 `MyWorkflow extends WorkflowEntrypoint`，通过 `env.FLOW.create` 创建实例。位于 `/client/v4/accounts/{account_id}/workflows` 的官方 Workflows API 管理 definition、version、instance、status 和 event。

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

下一步：[概念](/zh/workflows/concepts/)。
