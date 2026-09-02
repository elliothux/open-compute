# 示例

Workflow class 导出 `run`；Worker 用 binding 创建实例。外部 IO 放进 `step.do` 并做成幂等。不依赖 Cloudflare dashboard 查看实例。

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
  "workflows": [
    { "binding": "FLOW", "name": "flow", "class_name": "MyWorkflow" }
  ]
}
```

`create` / `get` / `step.do` / `status` 与 [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) 对齐。`class_name` 必须有。步骤状态存在本机 SQLite。创建 definition 见[上手](/zh/workflows/get-started/)。
