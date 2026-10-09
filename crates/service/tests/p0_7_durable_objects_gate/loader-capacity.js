import { DurableObject } from "cloudflare:workers";

export class Counter extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.boot = crypto.randomUUID();
    this.order = [];
  }
  fetch(request) {
    const url = new URL(request.url);
    if (request.headers.has("x-open-compute-fetch-admission"))
      throw new Error("private admission header exposed");
    if (url.searchParams.has("order"))
      this.order.push("fetch-second:start", "fetch-second:end");
    const count = this.ctx.storage.kv.get("count") ?? 0;
    if (url.pathname === "/increment")
      this.ctx.storage.kv.put("count", count + 1);
    if (url.pathname === "/alarm") this.ctx.storage.setAlarm(Date.now() + 1000);
    if (url.pathname === "/ws") {
      const pair = new WebSocketPair();
      this.ctx.acceptWebSocket(pair[1]);
      pair[1].serializeAttachment({ id: this.ctx.id.toString() });
      return new Response(null, { status: 101, webSocket: pair[0] });
    }
    return Response.json({
      count: this.ctx.storage.kv.get("count") ?? 0,
      alarmed: this.ctx.storage.kv.get("alarmed") ?? false,
      boot: this.boot,
      id: this.ctx.id.toString(),
    });
  }
  async ordered(name, hold) {
    this.order.push(name + ":start");
    if (hold) await scheduler.wait(hold);
    this.order.push(name + ":end");
  }
  orderValue() {
    return this.order;
  }
  alarm() {
    this.ctx.storage.kv.put("alarmed", true);
  }
  webSocketMessage(socket, message) {
    socket.send(
      JSON.stringify({
        message,
        count: this.ctx.storage.kv.get("count"),
        id: socket.deserializeAttachment().id,
      }),
    );
  }
  webSocketClose(socket, code, reason) {
    socket.close(code, reason);
  }
}

export default {
  async fetch(request, env) {
    const name = new URL(request.url).searchParams.get("name");
    const stub = env.OBJECTS.get(env.OBJECTS.idFromName(name));
    if (new URL(request.url).pathname === "/mixed") {
      const first = stub.ordered("rpc-first", 80);
      const second = stub
        .fetch("https://object.invalid/?order=second", {
          headers: { "x-open-compute-fetch-admission": "external-spoof" },
        })
        .then((response) => response.json());
      const third = stub.ordered("rpc-third", 0);
      await Promise.all([first, second, third]);
      return Response.json(
        (await stub.orderValue()).filter((value) => value.endsWith(":start")),
      );
    }
    return stub.fetch(request);
  },
};
