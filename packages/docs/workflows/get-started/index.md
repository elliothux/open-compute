# Workflows get started

A Workflow definition is created or updated through the official API when Wrangler deploys a Worker that exports the class. Bind it with standard configuration:

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

Export `MyWorkflow extends WorkflowEntrypoint` and use `env.FLOW.create` to start instances. The official Workflows API under `/client/v4/accounts/{account_id}/workflows` manages definitions, versions, instances, status, and events.

```sh
bun run oc types --config wrangler.jsonc
bun run oc deploy --config wrangler.jsonc
```

Next: [Concepts](/workflows/concepts/).
