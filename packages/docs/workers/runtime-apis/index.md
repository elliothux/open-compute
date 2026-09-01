# Runtime APIs

不要在本页粘贴 1,580 个成员签名。Worker 在本机 workerd isolate 里跑同一套 runtime 表面。

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    return new Response("ok");
  },
} satisfies ExportedHandler<Env>;
```

## 与 Cloudflare 相同

`fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC 与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 相同。完整成员去 Cloudflare 原文和 `ocd capabilities --json` 的 `products`。

## 故意不同

Workers runtime 是 `supported_with_deviation`（`OC-WKR-TCP-001`、`OC-WKR-LIMIT-001`）。Alarms、Version Metadata、WebSocket hibernation 为 `supported`。`ai`、`vectorize` 等非目标产品是 `unsupported`。不要从本表推导未列出的托管功能。


| 表面 | Cloudflare | 本平台 |
| --- | --- | --- |
| Handlers (`fetch`, `scheduled`, `queue`) | [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | [handlers](/workers/runtime-apis/handlers) |
| Bindings / `env` | [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | [bindings](/workers/runtime-apis/bindings) |
| Cache API | [Cache](https://developers.cloudflare.com/workers/runtime-apis/cache/) | [cache](/workers/runtime-apis/cache) |
| WebSockets | [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) | [websockets](/workers/runtime-apis/websockets)；hibernation 支持 |
| TCP sockets | [TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) | [tcp-sockets](/workers/runtime-apis/tcp-sockets)；`OC-WKR-TCP-001` |
| Node.js | [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | [nodejs](/workers/runtime-apis/nodejs) |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | [Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) | 相同；不在本站复述 |
| Durable Objects / Alarms | [Durable Objects](https://developers.cloudflare.com/durable-objects/api/) | 产品页；`OC-DO-001` |
| KV / R2 / D1 / Queues / Workflows | 各产品文档 | 本站对应产品；单机偏差见 [偏差](/platform/deviations) |

