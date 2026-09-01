# 上手

`ocd` 必须就绪。创建 namespace 需要已有的 Worker id 和 class 名：先部署带 class 的 Worker，再创建 namespace，再绑回去重新 `oc run`。

## 1. 写出 class 并第一次部署

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

记下返回的 `workerId`。

## 2. 创建 namespace

以下为本平台控制面。不提供 Cloudflare REST / `client.v4`。body 是 camelCase：`workerId`、`className`。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/durable-objects/namespaces" \
  -H "content-type: application/json" \
  -H "idempotency-key: do-create-1" \
  -d '{"name":"counters","workerId":"<workerId>","className":"Counter"}'
```

响应含 `resourceId`。

## 3. 绑定并再部署

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<resourceId>", "className": "Counter" }
  }
}
```

`className` 必须有。然后：

```sh
bun run oc types --config open-compute.json
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

把 default export 改成 `env.COUNTER.idFromName("global")` 再 `get` / `fetch`。CLI 为 `oc`，不是 Wrangler。下一步：[概念](/durable-objects/concepts/)。
