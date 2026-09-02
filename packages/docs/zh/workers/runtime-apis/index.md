# Runtime APIs

Worker 在本机 workerd isolate 中运行与 Cloudflare Workers 对齐的 runtime 表面。完整成员以 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 为准。

```ts
export default {
  fetch(request: Request, env: Env, ctx: ExecutionContext): Response {
    return new Response("ok");
  },
} satisfies ExportedHandler<Env>;
```

## 表面

| 表面 | Cloudflare | open-compute |
| --- | --- | --- |
| Handlers（`fetch`、`scheduled`、`queue`） | [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | [handlers](/zh/workers/runtime-apis/handlers) |
| Bindings / `env` | [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) | [bindings](/zh/workers/runtime-apis/bindings) |
| Cache API | [Cache](https://developers.cloudflare.com/workers/runtime-apis/cache/) | [cache](/zh/workers/runtime-apis/cache) |
| WebSockets | [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) | [websockets](/zh/workers/runtime-apis/websockets)；hibernation 可用 |
| TCP sockets | [TCP sockets](https://developers.cloudflare.com/workers/runtime-apis/tcp-sockets/) | [tcp-sockets](/zh/workers/runtime-apis/tcp-sockets) |
| Node.js | [Node.js compatibility](https://developers.cloudflare.com/workers/runtime-apis/nodejs/) | [nodejs](/zh/workers/runtime-apis/nodejs) |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | [Runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) | 对齐；本站不复述 |
| Durable Objects / Alarms | [Durable Objects](https://developers.cloudflare.com/durable-objects/api/) | 见 Durable Objects 产品页 |
| KV / R2 / D1 / Queues / Workflows | 各产品文档 | 本站对应产品页 |

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `fetch` / Request / Response / Streams / HTMLRewriter / Web Crypto / RPC | 是 | 是 |
| Alarms、Version Metadata、WebSocket hibernation | 是 | 是 |
| Workers AI / Vectorize 等非本平台产品 | 是 | 不提供 |
| 出站 TCP / `fetch` 网络策略 | Cloudflare 托管策略 | 见 [TCP sockets](/zh/workers/runtime-apis/tcp-sockets) |
| 请求级 CPU / subrequest 配额 | 是 | 见 [限制](/zh/workers/platform/limits) |

