# Examples

These samples actually run on this machine's workerd. Do not copy Cloudflare [geolocation / country-code](https://developers.cloudflare.com/workers/examples/geolocation-hello-world/) examples as if they were a global Anycast product: there is no `request.cf.country` edge-geo product here.

## Hello JSON

Same hello-worker as [Get started](/en/workers/get-started/).

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

The KV API matches Cloudflare. See [KV](/en/kv/). Authority is single-node SQLite (`OC-KV-001`), not global replication.

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
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` is an existing KV namespace on this platform, not a name placeholder.

## Cron `scheduled` handler

The Cron product lives at [Cron Triggers](/en/workers/configuration/cron-triggers). The handler matches [Cloudflare scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/).

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

Expressions are five UTC fields plus the documented local Quartz-like extensions. Recovery: `OC-CRON-001`. The platform deployment metadata field is `crons` (string array). `open-compute.json` has no Wrangler `triggers` key; adding one is an unknown field and fails.

## Service Binding `fetch`

Fetch / RPC between Workers in the same account. See [bindings](/en/workers/runtime-apis/bindings).

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

The target Worker name is resolved and frozen as a target ID at deploy time. There is no cross-region discovery (`OC-SERVICE-001`).

## Same as Cloudflare

Module handlers, KV `get`/`put`, `scheduled`, Service Binding `fetch`.

## Intentional delta

No geolocation samples as a global product; no `workers.dev`; all state lives on this machine. Cloudflare examples that depend on the edge, Analytics Engine, AI, or Turnstile are not capabilities of this platform.
