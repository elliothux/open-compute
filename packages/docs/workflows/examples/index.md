# Examples

Export a Workflow class with `run`; the Worker creates instances through the binding. Put external I/O in `step.do` and make it idempotent. Instance inspection does not depend on a Cloudflare dashboard.

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

`create` / `get` / `step.do` / `status` match [Cloudflare Workflows](https://developers.cloudflare.com/workflows/). `class_name` is required. Execution authority is local SQLite. Create the definition: [Get started](/workflows/get-started/).
