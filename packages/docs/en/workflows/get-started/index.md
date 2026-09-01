# Get started

`ocd` must be ready. Create a definition, bind the class, then `oc run`.

## 1. Create a definition

Local platform control plane, not Cloudflare REST / `client.v4`.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/workflows" \
  -H "content-type: application/json" \
  -H "idempotency-key: wf-create-1" \
  -d '{"name":"orders"}'
```

The response includes the definition `id`. Put it in the binding.

## 2. Bind

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<definition-id>", "className": "MyWorkflow" }
  }
}
```

`className` is required.

```sh
bun run oc types --config open-compute.json
```

## 3. Worker

```ts
export class MyWorkflow extends WorkflowEntrypoint<Env, { hello: string }> {
  async run(event: WorkflowEvent<{ hello: string }>, step: WorkflowStep) {
    return step.do("echo", async () => event.payload);
  }
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const instance = await env.FLOW.create({ params: { hello: "world" } });
    return Response.json({ id: instance.id });
  },
} satisfies ExportedHandler<Env>;
```

## 4. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/workflows/concepts/).
