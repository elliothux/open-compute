# Get started

`ocd` must already be ready, with S3 configured (`[s3]` plus `r2_prefix`). Create a logical bucket, bind it in `open-compute.json`, and run the Worker with `oc`. `oc run` does not start another workerd. See [ocd get started](/en/ocd/get-started) and [configuration](/en/ocd/configuration).

## 1. Create a logical bucket

The following is the platform control plane. Cloudflare REST and `client.v4` are not provided. Object bytes go to the configured S3 prefix, not SQLite.

```sh
ACCOUNT_ID=$(curl -sS http://127.0.0.1:8787/v1/account | python3 -c 'import json,sys; print(json.load(sys.stdin)["accountId"])')
curl -sS -X POST "http://127.0.0.1:8787/v1/accounts/$ACCOUNT_ID/r2/buckets" \
  -H "content-type: application/json" \
  -H "idempotency-key: r2-create-1" \
  -d '{"name":"my-bucket"}'
```

Success returns `{ "resourceId": "...", "state": "ready" }`.

## 2. Bind

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

## 4. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/r2/concepts/), [Guides](/en/r2/guides/).
