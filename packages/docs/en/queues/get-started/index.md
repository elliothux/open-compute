# Get started

Create a Queue, bind a producer in `open-compute.json`, and run the Worker with `oc`. `oc run` does not start another workerd. See [ocd get started](/en/ocd/get-started).

## 1. Create a Queue

The following is the platform control plane. Cloudflare REST and `client.v4` are not provided.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/queues" \
  -H "content-type: application/json" \
  -H "idempotency-key: queue-create-1" \
  -d '{"name":"jobs"}'
```

The response is `{ "queue": { "id": "...", ... } }`. Put `queue.id` in the binding.

## 2. Producer binding

```json
{
  "name": "queue-app",
  "main": "src/index.ts",
  "bindings": {
    "QUEUE": { "type": "queue_producer", "id": "<queue.id>" }
  }
}
```

Current `open-compute.json` has no Wrangler-style consumers array; unknown fields are rejected. The consumer is the Worker's exported `queue` handler. The platform delivers via a push consumer on the deployment.

```sh
bun run oc types --config open-compute.json
```

## 3. Worker

```ts
export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    await env.QUEUE.send({ hello: "world" });
    await env.QUEUE.sendBatch([{ body: { hello: "batch" } }]);
    return new Response("queued");
  },
  async queue(batch: MessageBatch<{ hello: string }>): Promise<void> {
    for (const message of batch.messages) {
      message.ack();
    }
  },
} satisfies ExportedHandler<Env>;
```

## 4. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/queues/concepts/).
