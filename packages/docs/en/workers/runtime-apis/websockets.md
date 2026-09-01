# WebSockets

Standard WebSocket upgrade, plus hibernation on Durable Objects. Hibernation is `supported` in capabilities (19 members), not a deviation.

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

## Same as Cloudflare

`WebSocketPair`, `accept()`, messages and close. Hibernation accept/tags/get, auto-response, and attachment reconstruction have runtime evidence.

## Intentional delta

Connections land on this machine's one workerd. No global-edge upgrade, no Cloudflare duration billing. DO placement is still `OC-DO-001`.
