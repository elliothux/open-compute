# Examples

Export a Workflow class with `run`; the Worker creates instances through the binding. Put external I/O in `step.do` and make it idempotent. Do not depend on a Cloudflare dashboard to inspect instances.

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

## Same as Cloudflare

`create` / `get` / `step.do` / `status` match [Cloudflare Workflows](https://developers.cloudflare.com/workflows/). `className` is required.

## Intentional differences: OC-WORKFLOW-001

Execution authority is local SQLite. Callbacks are at-least-once until commit; completed steps skip on replay; external side effects do not roll back with the snapshot. Create the definition: [Get started](/en/workflows/get-started/).
