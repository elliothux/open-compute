# Get started

`ocd` must be ready. Creating a namespace needs an existing Worker id and class name: deploy the Worker that exports the class, create the namespace, bind it, and `oc run` again.

## 1. Write the class and deploy once

```ts
export class Counter {
  constructor(private readonly ctx: DurableObjectState) {}
  async fetch(): Promise<Response> {
    const n = ((await this.ctx.storage.get<number>("n")) ?? 0) + 1;
    await this.ctx.storage.put("n", n);
    return Response.json({ n });
  }
}

export default {
  fetch(): Response {
    return new Response("deploy the class first");
  },
} satisfies ExportedHandler;
```

```json
{
  "name": "do-app",
  "main": "src/index.ts"
}
```

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd> --json
```

Keep the returned `workerId`.

## 2. Create a namespace

The following is the platform control plane. Cloudflare REST and `client.v4` are not provided. The body is camelCase: `workerId`, `className`.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/durable-objects/namespaces" \
  -H "content-type: application/json" \
  -H "idempotency-key: do-create-1" \
  -d '{"name":"counters","workerId":"<workerId>","className":"Counter"}'
```

The response includes `resourceId`.

## 3. Bind and deploy again

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<resourceId>", "className": "Counter" }
  }
}
```

`className` is required. Then:

```sh
bun run oc types --config open-compute.json
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

Change the default export to `env.COUNTER.idFromName("global")` then `get` / `fetch`. The CLI is `oc`, not Wrangler. Next: [Concepts](/durable-objects/concepts/).
