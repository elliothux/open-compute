# Runtime APIs

Do not paste 1,580 member signatures on this page. Workers run the same runtime surface in this machine's workerd isolate.

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    return new Response("ok");
  },
} satisfies ExportedHandler<Env>;
```

## Same as Cloudflare

`fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC match [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/). Full members: Cloudflare pages and `ocd capabilities --json` `products`.

## Intentional delta

The Workers runtime is `supported_with_deviation` (`OC-WKR-TCP-001`, `OC-WKR-LIMIT-001`). Alarms, Version Metadata, and WebSocket hibernation are `supported`. Non-target products such as `ai` and `vectorize` are `unsupported`. Do not infer unlisted hosted features from this table.


| Surface | Cloudflare | This platform |
| --- | --- | --- |
| Handlers (`fetch`, `scheduled`, `queue`) | [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | [handlers](/en/workers/runtime-apis/handlers) |
| Bindings / `env` | [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | [bindings](/en/workers/runtime-apis/bindings) |
| Cache API | [Cache](https://developers.cloudflare.com/workers/runtime-apis/cache/) | [cache](/en/workers/runtime-apis/cache) |
| WebSockets | [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) | [websockets](/en/workers/runtime-apis/websockets); hibernation supported |
| TCP sockets | [TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) | [tcp-sockets](/en/workers/runtime-apis/tcp-sockets); `OC-WKR-TCP-001` |
| Node.js | [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | [nodejs](/en/workers/runtime-apis/nodejs) |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | [Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) | Same; not restated here |
| Durable Objects / Alarms | [Durable Objects](https://developers.cloudflare.com/durable-objects/api/) | Product pages; `OC-DO-001` |
| KV / R2 / D1 / Queues / Workflows | Each product's docs | Matching product pages here; single-node deviations in [Deviations](/en/platform/deviations) |

