# 示例

计数器见[概述](/durable-objects/)。Hibernation 在本地这一个 workerd 上运行。

## 计数器

见[概述](/durable-objects/)的 `Counter` 示例。

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

完整 hibernation API 见 [WebSockets](/workers/runtime-apis/websockets)。Alarms 示例见 [alarms](/durable-objects/alarms)。
