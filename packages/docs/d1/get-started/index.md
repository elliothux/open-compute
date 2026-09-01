# 上手

先让 `ocd` 处于 ready。`oc run` 不会再起 workerd。平台还没起来时看 [ocd 上手](/ocd/get-started)。

## 1. 创建 database

本机平台控制面，不是 Cloudflare REST / `client.v4`。

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/d1/databases" \
  -H "content-type: application/json" \
  -H "idempotency-key: d1-create-1" \
  -d '{"name":"my-db"}'
```

成功返回 `{ "resourceId": "...", "state": "ready" }`。

## 2. 绑定

```json
{
  "name": "d1-app",
  "main": "src/index.ts",
  "bindings": {
    "DB": { "type": "d1_database", "id": "<resourceId>" }
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
    await env.DB.exec(
      "CREATE TABLE IF NOT EXISTS Customers (CustomerId INTEGER PRIMARY KEY, CompanyName TEXT);",
    );
    await env.DB.prepare(
      "INSERT INTO Customers (CompanyName) VALUES (?)",
    ).bind("Bs Beverages").run();
    const { results } = await env.DB.prepare(
      "SELECT * FROM Customers WHERE CompanyName = ?",
    ).bind("Bs Beverages").all();
    return Response.json(results);
  },
} satisfies ExportedHandler<Env>;
```

## 4. 跑起来

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 是 `oc`，不是 Wrangler。下一步：[概念](/d1/concepts/)、[指南](/d1/guides/)。
