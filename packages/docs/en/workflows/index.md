# Workflows

Workflows are replayable multi-step applications. Execution authority is local SQLite. Callbacks are at-least-once until their result commits; replay skips callbacks that already committed. There is no cross-region execution and no Cloudflare dashboard / observability.

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

## Same as Cloudflare

The binding / instance API matches [Cloudflare Workflows](https://developers.cloudflare.com/workflows/): `create` / `get` / `createBatch` / `deleteBatch`, `step.do` / sleep / event, status / pause / resume / terminate / restart. 72 target members are `supported_with_deviation`.

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<workflow-id>", "className": "MyWorkflow" }
  }
}
```

Workflow bindings must include `className`. Optional `schedules` is a string array. Grammar: [bindings](/en/workers/configuration/bindings). Do not copy Cloudflare REST or the dashboard from this page.

## Intentional differences

**`OC-WORKFLOW-001`**: Workflow execution uses local SQLite authority. Callbacks are at-least-once until their result commits; replay skips durably completed callbacks; external product effects do not roll back with Workflow snapshots. The platform does not claim cross-region execution, global placement, or Cloudflare dashboard/observability.

Full text: [Deviations](/en/workflows/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/workflows/get-started/)
- [Concepts](/en/workflows/concepts/)
- [Guides](/en/workflows/guides/)
- [Examples](/en/workflows/examples/)
- [Limits](/en/workflows/platform/limits)
- [Deviations](/en/workflows/platform/deviations)
