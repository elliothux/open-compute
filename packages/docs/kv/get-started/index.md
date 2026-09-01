# 上手

先让 `ocd` 处于 ready，再创建 namespace、绑定、用 `oc` 跑 Worker。`oc run` 不会再起一个 workerd。平台还没起来时，先看 [ocd 上手](/ocd/get-started)。

## 1. 创建 namespace

资源必须先在平台上存在；写进 `open-compute.json` 不会创建 KV。下面是**本机平台控制面**，不是 Cloudflare REST，也不是 `client.v4`。

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

普通产品 binding 是 `{type, id, permissions?}`。可选 `permissions`：`{ "read": true, "write": true }`。语法见 [bindings](/workers/configuration/bindings)。

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

## 4. 跑起来

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 是 `oc`，不是 Wrangler。下一步：[概念](/kv/concepts/)、[指南](/kv/guides/)。
