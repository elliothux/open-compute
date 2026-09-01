# 示例

Workflow class 导出 `run`；Worker 用 binding 创建实例。外部 IO 放进 `step.do` 并做成幂等。不要依赖 Cloudflare dashboard 看实例。

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

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  }
}
```

## 与 Cloudflare 相同

`create` / `get` / `step.do` / `status` 与 [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) 相同。`className` 必须有。

## 故意不同：OC-WORKFLOW-001

执行权威是本地 SQLite。callback 在提交前是 at-least-once；已完成的 step 在 replay 时跳过；外部副作用不会随 snapshot 回滚。创建 definition 见[上手](/workflows/get-started/)。
