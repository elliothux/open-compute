import { DurableObject } from "cloudflare:workers";
import unsafe from "workerd:unsafe";

function assert(value, message) {
  if (!value) throw new Error(message);
}

export class Room extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    const constructs = (this.ctx.storage.kv.get("constructs") || 0) + 1;
    this.ctx.storage.kv.put("constructs", constructs);
  }

  async fetch(request) {
    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    this.ctx.acceptWebSocket(server, ["echo"]);
    server.serializeAttachment({ n: 1, tag: "echo" });
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws, message) {
    const handled = (this.ctx.storage.kv.get("handled") || 0) + 1;
    this.ctx.storage.kv.put("handled", handled);
    if (message === "boom") throw new Error("fixture-ws-error");
    this.ctx.storage.kv.put("attachment", ws.deserializeAttachment());
    ws.send(message);
  }

  async webSocketClose(_ws, code, reason, wasClean) {
    this.ctx.storage.kv.put("ws-close", { code, reason, wasClean });
  }

  async webSocketError(_ws, error) {
    this.ctx.storage.kv.put("ws-error", String(error));
  }

  async inspect() {
    const sockets = this.ctx.getWebSockets("echo");
    const first = sockets[0];
    const pair = this.ctx.getWebSocketAutoResponse();
    return {
      constructs: this.ctx.storage.kv.get("constructs") || 0,
      handled: this.ctx.storage.kv.get("handled") || 0,
      sockets: sockets.length,
      tags: first ? this.ctx.getTags(first) : [],
      attachment: first ? first.deserializeAttachment() : this.ctx.storage.kv.get("attachment"),
      autoRequest: pair ? pair.request : null,
      autoResponse: pair ? pair.response : null,
      close: this.ctx.storage.kv.get("ws-close") || null,
      error: this.ctx.storage.kv.get("ws-error") || null,
    };
  }
}

function nextMessage(ws) {
  return Promise.race([
    new Promise((resolve, reject) => {
      ws.addEventListener("message", event => resolve(event.data), { once: true });
      ws.addEventListener("error", reject, { once: true });
    }),
    scheduler.wait(5000).then(() => { throw new Error("websocket timeout"); }),
  ]);
}

function nextClose(ws) {
  return new Promise((resolve, reject) => {
    ws.addEventListener("close", event => resolve(event), { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
}

export default {
  async test(_controller, env) {
    const stub = env.ROOMS.getByName("room");
    const opened = await stub.fetch("https://room.invalid/", {
      headers: { Upgrade: "websocket", Connection: "Upgrade" },
    });
    const ws = opened.webSocket;
    assert(ws, "missing hibernatable websocket");
    ws.accept();
    const before = await stub.inspect();
    assert(before.constructs === 1, `expected 1 construct before eviction, got ${before.constructs}`);
    assert(before.sockets === 1, "socket missing before eviction");
    assert(Array.isArray(before.tags) && before.tags.includes("echo"), "tags missing before eviction");
    assert(before.attachment?.n === 1 && before.attachment?.tag === "echo", "attachment missing before eviction");

    await unsafe.evict(stub);

    const autoWait = nextMessage(ws);
    ws.send("ping");
    const auto = await autoWait;
    assert(auto === "pong", `auto-response while evicted returned ${auto}`);

    const echoed = nextMessage(ws);
    ws.send("hello");
    assert(await echoed === "hello", "reconstructed handler did not echo");

    const afterMessage = await stub.inspect();
    assert(afterMessage.constructs === 2, `expected exactly one reconstruction, got ${afterMessage.constructs}`);
    assert(afterMessage.handled === 1, `auto-response must not run the handler, handled=${afterMessage.handled}`);
    assert(afterMessage.sockets === 1, "hibernated socket was not restored");
    assert(Array.isArray(afterMessage.tags) && afterMessage.tags.includes("echo"), "tags were not restored");
    assert(afterMessage.attachment?.n === 1 && afterMessage.attachment?.tag === "echo", "attachment was not restored");
    assert(afterMessage.autoRequest === "ping" && afterMessage.autoResponse === "pong", "auto-response pair missing");

    const closed = nextClose(ws);
    ws.close(1000, "done");
    await closed;
    const afterClose = await stub.inspect();
    assert(afterClose.close?.code === 1000, `close handler missing: ${JSON.stringify(afterClose.close)}`);
    assert(afterClose.sockets === 0, "closed socket still listed");

  },
};
