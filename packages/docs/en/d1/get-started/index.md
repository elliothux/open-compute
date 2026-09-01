# Get started

Have `ocd` ready. `oc run` does not start another workerd. If the platform is not up, see [ocd get started](/en/ocd/get-started).

## 1. Create a database

Local platform control plane, not Cloudflare REST / `client.v4`.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/d1/databases" \
  -H "content-type: application/json" \
  -H "idempotency-key: d1-create-1" \
  -d '{"name":"my-db"}'
```

Success returns `{ "resourceId": "...", "state": "ready" }`.

## 2. Bind

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

## 4. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/d1/concepts/), [Guides](/en/d1/guides/).
