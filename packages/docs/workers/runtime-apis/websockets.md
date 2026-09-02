# WebSockets

Standard WebSocket upgrade, plus hibernation on Durable Objects.

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

DO hibernation: `state.acceptWebSocket`, tags, `webSocketMessage` / `webSocketClose` / `webSocketError`, serialize/deserialize attachment. See [WebSockets](https://developers.cloudflare.com/workers/runtime-apis/websockets/) and the Durable Objects hibernation API.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `WebSocketPair`, `accept()`, messages and close | Yes | Yes |
| Hibernation: accept / tags / get, auto-response, attachment reconstruction | Yes | Yes |
| Where connections land | Global-edge upgrade | One workerd on this node |
| Duration billing | Yes | Not provided |
| Durable Object placement | Global placement | This node |

