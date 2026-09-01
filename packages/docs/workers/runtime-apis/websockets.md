# WebSockets

标准 WebSocket 升级，以及 Durable Object 上的 hibernation。

```ts
export default {
  async fetch(request: Request): Promise<Response> {
    if (request.headers.get("Upgrade") !== "websocket") {
      return new Response("expected websocket", { status: 426 });
    }
    const pair = new WebSocketPair();
    pair[1].accept();
    pair[1].send("hello");
    return new Response(null, { status: 101, webSocket: pair[0] });
  },
} satisfies ExportedHandler;
```

DO hibernation：`state.acceptWebSocket`、tags、`webSocketMessage` / `webSocketClose` / `webSocketError`、serialize/deserialize attachment。对照 [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) 与 Durable Objects 的 hibernation API。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `WebSocketPair`、`accept()`、消息与关闭 | 是 | 是 |
| hibernation：accept / tags / get、auto-response、attachment 重建 | 是 | 是 |
| 连接落地 | 全球边缘升级 | 该节点上的一个 workerd |
| duration 计费 | 是 | 不提供 |
| Durable Object 位置 | 全球 placement | 该节点 |

