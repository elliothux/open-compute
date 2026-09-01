# Workflows

Workflows 是可重放的多步应用。执行权威是本地 SQLite。callback 在结果提交前是 at-least-once；已耐久完成的 callback 在 replay 时跳过。没有跨地域执行，没有 Cloudflare dashboard / observability。

```ts
export class MyWorkflow extends WorkflowEntrypoint<Env, { hello: string }> {
  async run(event: WorkflowEvent<{ hello: string }>, step: WorkflowStep) {
    const first = await step.do("first", async () => {
      return { ok: true, hello: event.payload.hello };
    });
    return first;
  }
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const instance = await env.FLOW.create({ params: { hello: "world" } });
    return Response.json({ id: instance.id, status: await instance.status() });
  },
} satisfies ExportedHandler<{ FLOW: Workflow }>;
```

## 与 Cloudflare 相同

Binding / instance API 与 [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) 相同：`create` / `get` / `createBatch` / `deleteBatch`、`step.do` / sleep / event、status / pause / resume / terminate / restart。72 个目标成员为 `supported_with_deviation`。

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  }
}
```

Workflow 必须提供 `className`。可选 `schedules` 字符串数组。语法见 [bindings](/workers/configuration/bindings)。不要从本页抄 Cloudflare REST 或 dashboard。

## 故意不同

**`OC-WORKFLOW-001`**：Workflow 在本地 SQLite authority 上执行。callback 在结果提交前是 at-least-once；replay 会跳过已耐久完成的 callback；外部产品副作用不会随 Workflow snapshot 回滚。不声称跨地域执行、全球 placement 或 Cloudflare dashboard/observability。

全文见 [偏差](/workflows/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/workflows/get-started/)
- [概念](/workflows/concepts/)
- [指南](/workflows/guides/)
- [示例](/workflows/examples/)
- [限制](/workflows/platform/limits)
- [偏差](/workflows/platform/deviations)
