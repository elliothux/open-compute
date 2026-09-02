# 上手

创建 namespace，在 `open-compute.json` 中绑定，再用 `oc` 运行 Worker。`oc run` 不会再起一个 workerd。若平台尚未就绪，见 [ocd 上手](/zh/ocd/get-started)。

## 1. 创建 namespace

资源必须先在open-compute 上存在；写入 `open-compute.json` 不会创建 KV。以下为本平台控制面。不提供 Cloudflare REST / `client.v4`。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
# 若 admin 监听需要认证，加 Authorization: Bearer $OPEN_COMPUTE_ADMIN_TOKEN
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/kv/namespaces" \
  -H "content-type: application/json" \
  -H "idempotency-key: kv-create-1" \
  -d '{"name":"my-kv"}'
```

成功时返回 `{ "resourceId": "...", "state": "ready" }`。把 `resourceId` 填进项目配置。

## 2. 绑定

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<resourceId>" }
  }
}
```

普通产品 binding 为 `{ type, id, permissions? }`。可选 `permissions`：`{ "read": true, "write": true }`。语法见 [bindings](/zh/workers/configuration/bindings)。

```sh
bun run oc types --config open-compute.json
```

## 3. Worker

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (request.method === "PUT") {
      await env.KV.put("hello", await request.text());
      return new Response("ok");
    }
    if (request.method === "DELETE") {
      await env.KV.delete("hello");
      return new Response("ok");
    }
    const url = new URL(request.url);
    if (url.pathname === "/list") {
      return Response.json(await env.KV.list({ prefix: "hello" }));
    }
    return new Response((await env.KV.get("hello")) ?? "missing");
  },
} satisfies ExportedHandler<Env>;
```

## 4. 运行

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 为 `oc`，不是 Wrangler。下一步：[概念](/zh/kv/concepts/)、[指南](/zh/kv/guides/)。
