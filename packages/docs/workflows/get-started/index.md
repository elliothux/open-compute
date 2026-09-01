# 上手

创建 definition，绑定 class，再用 `oc` 运行。`ocd` 必须就绪。

## 1. 创建 definition

以下为本平台控制面。不提供 Cloudflare REST / `client.v4`。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/workflows" \
  -H "content-type: application/json" \
  -H "idempotency-key: wf-create-1" \
  -d '{"name":"orders"}'
```

响应含 definition 的 `id`。把它填进 binding。

## 2. 绑定

```json
{
  "name": "flow-app",
  "main": "src/index.ts",
  "bindings": {
    "FLOW": { "type": "workflow", "id": "<definition-id>", "className": "MyWorkflow" }
  }
}
```

`className` 必须有。

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

## 4. 运行

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 为 `oc`，不是 Wrangler。下一步：[概念](/workflows/concepts/)。
