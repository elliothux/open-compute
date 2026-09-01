# 上手

`ocd` 必须已经 ready，并且配置了 S3（`[s3]` + `r2_prefix`）。`oc run` 不会再起 workerd。见 [ocd 上手](/ocd/get-started) 和 [配置](/ocd/configuration)。

## 1. 创建逻辑 bucket

本机平台控制面，不是 Cloudflare REST / `client.v4`。对象字节进你配置的 S3 prefix，不进 SQLite。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/r2/buckets" \
  -H "content-type: application/json" \
  -H "idempotency-key: r2-create-1" \
  -d '{"name":"my-bucket"}'
```

成功返回 `{ "resourceId": "...", "state": "ready" }`。

## 2. 绑定

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<resourceId>" }
  }
}
```

```sh
bun run oc types --config open-compute.json
```

## 3. Worker

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === "PUT") {
      await env.BUCKET.put("hello", request.body);
      return new Response("ok");
    }
    const object = await env.BUCKET.get("hello");
    if (!object) return new Response("missing", { status: 404 });
    return new Response(object.body);
  },
} satisfies ExportedHandler<Env>;
```

## 4. 跑起来

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 是 `oc`，不是 Wrangler。下一步：[概念](/r2/concepts/)、[指南](/r2/guides/)。
