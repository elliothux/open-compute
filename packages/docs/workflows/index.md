# Workflows

Workflows are replayable multi-step applications. Execution authority is local SQLite on the node running ocd.

For example, you can use Workflows for:

- Multi-step applications with durable steps
- Sleep and wait for events
- Replay after interruption

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

Bind in `wrangler.jsonc` with Wrangler's standard Workflow field:

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "workflows": [
    { "binding": "FLOW", "name": "flow", "class_name": "MyWorkflow" }
  ]
}
```

Grammar: [bindings](/workers/configuration/bindings). Pinned Wrangler owns Workflow definition deployment; the official SDK owns instances and lifecycle operations.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Binding / instance API | [Cloudflare Workflows](https://developers.cloudflare.com/workflows/) | Same: `create` / `get` / `createBatch` / `deleteBatch`, `step.do` / sleep / event, status / pause / resume / terminate / restart |
| Execution | Cross-region | Local SQLite on the node running ocd |
| Callbacks | — | At-least-once until result commit; replay skips durable-complete callbacks |
| External side effects | — | Do not roll back with Workflow snapshots |
| Dashboard / observability | Available | Not provided |
| Binding | Wrangler | Standard `workflows[].binding/name/class_name`; `class_name` required |

## Next

- [Get started](/workflows/get-started/)
- [Concepts](/workflows/concepts/)
- [Guides](/workflows/guides/)
- [Examples](/workflows/examples/)
- [Limits](/workflows/platform/limits)
- [Behavior differences](/workflows/platform/deviations)
