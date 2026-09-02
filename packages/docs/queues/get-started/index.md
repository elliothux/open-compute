# 上手

创建 Queue，在 `open-compute.json` 中绑定 producer，再用 `oc` 运行 Worker。`oc run` 不会再起一个 workerd。见 [ocd 上手](/ocd/get-started)。

## 1. 创建 Queue

以下为本平台控制面。不提供 Cloudflare REST / `client.v4`。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/queues" \
  -H "content-type: application/json" \
  -H "idempotency-key: queue-create-1" \
  -d '{"name":"jobs"}'
```

响应是 `{ "queue": { "id": "...", ... } }`。把 `queue.id` 填进 binding。

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

当前 `open-compute.json` 没有 Wrangler 风格的 consumers 数组；未知字段会拒绝。Consumer 是 Worker 导出的 `queue` handler。平台按 deployment 上的 push consumer 投递。

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

## 4. 运行

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 为 `oc`，不是 Wrangler。下一步：[概念](/queues/concepts/)。
