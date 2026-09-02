# Runtime APIs

Workers run the Cloudflare-aligned runtime surface in a workerd isolate on this node. For identical APIs, use [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/).

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    return new Response("ok");
  },
} satisfies ExportedHandler<Env>;
```

## Surfaces

| Surface | Cloudflare | open-compute |
| --- | --- | --- |
| Handlers (`fetch`, `scheduled`, `queue`) | [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | [handlers](/workers/runtime-apis/handlers) |
| Bindings / `env` | [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | [bindings](/workers/runtime-apis/bindings) |
| Cache API | [Cache](https://developers.cloudflare.com/workers/runtime-apis/cache/) | [cache](/workers/runtime-apis/cache) |
| WebSockets | [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) | [websockets](/workers/runtime-apis/websockets); hibernation available |
| TCP sockets | [TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) | [tcp-sockets](/workers/runtime-apis/tcp-sockets) |
| Node.js | [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | [nodejs](/workers/runtime-apis/nodejs) |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | [Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) | Aligned; not restated here |
| Durable Objects / Alarms | [Durable Objects](https://developers.cloudflare.com/durable-objects/api/) | Durable Objects product pages |
| KV / R2 / D1 / Queues / Workflows | Each product's docs | Matching product pages on this site |

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | Yes | Yes |
| Alarms, Version Metadata, WebSocket hibernation | Yes | Yes |
| Workers AI / Vectorize and other non-platform products | Yes | Not provided |
| Outbound TCP / `fetch` network policy | Cloudflare hosted policy | See [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| Request-scoped CPU / subrequest quotas | Yes | See [Limits](/workers/platform/limits) |

