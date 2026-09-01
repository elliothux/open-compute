# WebSockets

标准 WebSocket 升级，加上 Durable Object 上的 hibernation。hibernation 在 capability 里是 `supported`（19 个成员），不是偏差项。

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

DO hibernation：`state.acceptWebSocket`、tags、`webSocketMessage` / `webSocketClose` / `webSocketError`、serialize/deserialize attachment。对照 [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) 和 Durable Objects 的 hibernation API。

## 与 Cloudflare 相同

`WebSocketPair`、`accept()`、消息与关闭。hibernation 的 accept/tags/get、auto-response、attachment 重建均有 runtime 证据。

## 故意不同

连接落在这一台机器这一个 workerd 上。没有全球边缘升级、没有 Cloudflare 的 duration 计费。DO 位置仍受 `OC-DO-001` 约束。
