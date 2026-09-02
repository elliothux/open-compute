# Examples

These samples run on workerd on this node. Cloudflare [geolocation / country-code](https://developers.cloudflare.com/workers/examples/geolocation-hello-world/) examples depend on global Anycast and `request.cf.country`. That edge-geo product is not provided.

## Hello JSON

Same hello-worker as [Get started](/workers/get-started/).

```ts
export default {
  fetch(request: Request, env: Env): Response {
    return Response.json({
      message: env.GREETING,
      pathname: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": { "GREETING": "Hello from TypeScript" }
}
```

## KV get / put

The Worker-side KV API matches [KV](/kv/). Authority is a SQLite database on this node. Global replication is not provided.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    if (request.method === "PUT") {
      await env.KV.put("hello", await request.text());
      return new Response("ok");
    }
    return new Response((await env.KV.get("hello")) ?? "missing");
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "kv-demo",
  "main": "src/index.ts",
  "kv_namespaces": [{ "binding": "KV", "id": "<kv-namespace-id>" }]
}
```

`id` is an existing KV namespace on the platform, not a name placeholder.

## Cron `scheduled` handler

The Cron product lives at [Cron Triggers](/workers/configuration/cron-triggers). The handler matches [Cloudflare scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/).

```ts
export default {
  fetch(): Response {
    return new Response("cron worker");
  },
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    console.log(controller.cron, controller.scheduledTime);
  },
} satisfies ExportedHandler<Env>;
```

Expressions use the standard UTC cron syntax accepted by Wrangler. Declare them through the `triggers.crons` string array.

## Service Binding `fetch`

Fetch / RPC between Workers in the same platform account. See [bindings](/workers/runtime-apis/bindings).

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    return env.UPSTREAM.fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "front",
  "main": "src/index.ts",
  "services": [
    { "binding": "UPSTREAM", "service": "hello-typescript" }
  ]
}
```

The target Worker name is resolved and frozen as a target ID at deploy time. Service Bindings are same-platform only. Cross-region discovery is not provided.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Module handlers, KV `get`/`put`, `scheduled`, Service Binding `fetch` | Yes | Yes |
| `request.cf.country` edge geo / geolocation samples | Yes | Not provided |
| workers.dev | Yes | Not provided |
| Analytics Engine / Workers AI / Turnstile samples | Yes | Not provided |
| Where state lives | Global replication products | This node |

Next: [Configuration](/workers/configuration/), [Runtime APIs](/workers/runtime-apis/).
