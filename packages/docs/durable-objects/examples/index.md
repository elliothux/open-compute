# Examples

The counter sample is on the [overview](/durable-objects/). Hibernation runs on the single local workerd.

## Counter

See the `Counter` sample on the [overview](/durable-objects/).

## Hibernation WebSocket

```ts
export class Room {
  constructor(private readonly ctx: DurableObjectState) {}
  async fetch(request: Request): Promise<Response> {
    const pair = new WebSocketPair();
    this.ctx.acceptWebSocket(pair[1]);
    return new Response(null, { status: 101, webSocket: pair[0] });
  }
  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    ws.send(typeof message === "string" ? message : "bin");
  }
}
```

Full hibernation API: [WebSockets](/workers/runtime-apis/websockets). Alarm sample: [alarms](/durable-objects/alarms).
