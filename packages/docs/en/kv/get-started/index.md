# Get started

Create a namespace, bind it in `open-compute.json`, and run the Worker with `oc`. `oc run` does not start another workerd. If the platform is not up, start with [ocd get started](/en/ocd/get-started).

## 1. Create a namespace

The resource must already exist on the platform; writing `open-compute.json` does not create KV. The following is the platform control plane. Cloudflare REST and `client.v4` are not provided.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
# If the admin listener requires auth, add Authorization: Bearer $OPEN_COMPUTE_ADMIN_TOKEN
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/kv/namespaces" \
  -H "content-type: application/json" \
  -H "idempotency-key: kv-create-1" \
  -d '{"name":"my-kv"}'
```

Success returns `{ "resourceId": "...", "state": "ready" }`. Put `resourceId` in the project config.

## 2. Bind

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<resourceId>" }
  }
}
```

Ordinary product bindings are `{ type, id, permissions? }`. Optional `permissions`: `{ "read": true, "write": true }`. Grammar: [bindings](/en/workers/configuration/bindings).

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

## 4. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/kv/concepts/), [Guides](/en/kv/guides/).
