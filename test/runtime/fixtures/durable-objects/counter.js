import { DurableObject } from "cloudflare:workers";

function read(sql) {
  const rows = sql.exec("SELECT value FROM counter WHERE id = 1").toArray();
  return rows.length ? Number(rows[0].value) : 0;
}
function increment(sql) {
  sql.exec("INSERT INTO counter(id, value) VALUES(1, 1) ON CONFLICT(id) DO UPDATE SET value = value + 1");
  return read(sql);
}

export class Counter extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
  }
  async fetch(request) {
    const url = new URL(request.url);
    for (const name of [
      "x-open-compute-binding-token",
      "x-open-compute-account-id",
      "x-open-compute-worker-id",
      "x-open-compute-binding-id",
      "x-open-compute-deployment-id",
      "x-open-compute-descriptor-sha256",
      "x-open-compute-worker-code-sha256",
      "x-open-compute-route-generation",
      "x-open-compute-namespace-resource-id",
      "x-open-compute-object-id",
      "x-open-compute-object-generation",
      "x-open-compute-class-name",
      "x-open-compute-do-operation",
      "x-open-compute-request-id",
      "x-open-compute-startup-generation",
    ]) {
      if (request.headers.has(name)) return new Response("internal header leaked", { status: 500 });
    }
    if (url.searchParams.get("websocket") === "1") {
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      server.accept();
      server.addEventListener("message", async event => {
        const value = event.data instanceof Blob ? await event.data.arrayBuffer() : event.data;
        server.send(value);
      });
      return new Response(null, { status: 101, webSocket: client });
    }
    const hold = Number(url.searchParams.get("hold") || 0);
    if (hold > 0) await scheduler.wait(hold);
    const value = this.ctx.storage.transactionSync(() => increment(this.ctx.storage.sql));
    await this.ctx.storage.sync();
    return new Response(`${this.env.RELEASE}:${value}`);
  }
  async committedFailure() {
    increment(this.ctx.storage.sql);
    await this.ctx.storage.sync();
    throw new Error("fixture-write-confirmed-response-failed");
  }
  async heldWrite() {
    increment(this.ctx.storage.sql);
    await this.ctx.storage.sync();
    this.held = true;
    await scheduler.wait(60000);
  }
  async heldStatus() { return this.held === true; }
  async getValue() { return { release: this.env.RELEASE, value: read(this.ctx.storage.sql) }; }
  async echoBinary(value) { return value; }
  async rollback() {
    const before = read(this.ctx.storage.sql);
    try {
      this.ctx.storage.transactionSync(() => { increment(this.ctx.storage.sql); throw new Error("rollback"); });
    } catch {}
    return { rolledBack: read(this.ctx.storage.sql) === before, value: read(this.ctx.storage.sql) };
  }
  async storageMatrix() {
    const result = {};
    try {
      this.ctx.storage.kv.put("sync", { value: 1 });
      const value = this.ctx.storage.kv.get("sync");
      const listed = [...this.ctx.storage.kv.list()].some(([key]) => key === "sync");
      result.syncKv = value.value === 1 && listed && this.ctx.storage.kv.delete("sync");
    } catch { return { failedStage: "syncKv" }; }
    try {
      await this.ctx.storage.put("async", { value: 2 });
      const value = await this.ctx.storage.get("async");
      const listed = (await this.ctx.storage.list()).has("async");
      result.asyncKv = value.value === 2 && listed && await this.ctx.storage.delete("async");
    } catch { return { failedStage: "asyncKv" }; }
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("rolled-back", 1);
        throw new Error("rollback");
      });
    } catch {
      result.asyncTransactionRollback = await this.ctx.storage.get("rolled-back") === undefined;
    }
    try {
      result.blockConcurrency = await this.ctx.blockConcurrencyWhile(async () => true);
    } catch { return { failedStage: "blockConcurrency" }; }
    try {
      const waited = Promise.resolve(true);
      this.ctx.waitUntil(waited);
      result.waitUntil = await waited;
    } catch { return { failedStage: "waitUntil" }; }
    try {
      await this.ctx.storage.put("delete-all", true);
      await this.ctx.storage.deleteAll();
      result.deleteAll = this.ctx.storage.kv.get("delete-all") === undefined
        && await this.ctx.storage.get("delete-all") === undefined;
      this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
    } catch (error) { return { failedStage: "deleteAll", detail: String(error) }; }
    return result;
  }
  async ordered(label, hold) {
    const order = this.ctx.storage.kv.get("order") || [];
    order.push(`${label}:start`);
    this.ctx.storage.kv.put("order", order);
    if (hold > 0) await scheduler.wait(hold);
    const current = this.ctx.storage.kv.get("order") || [];
    current.push(`${label}:end`);
    this.ctx.storage.kv.put("order", current);
    return current;
  }
  async orderValue() { return this.ctx.storage.kv.get("order") || []; }
}

export class OtherCounter extends Counter {}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const name = url.searchParams.get("name") || "alpha";
    if (url.pathname === "/ids") {
      const named = env.COUNTER.idFromName("alpha");
      const originalTextEncoder = globalThis.TextEncoder;
      globalThis.TextEncoder = class { encode() { throw new Error("mutated"); } };
      const mutatedIntrinsicNamed = env.COUNTER.idFromName("alpha").toString();
      globalThis.TextEncoder = originalTextEncoder;
      let crossNamespaceRejected = false;
      try { env.OTHER.idFromString(named.toString()); } catch { crossNamespaceRejected = true; }
      let uppercaseRejected = false;
      try { env.COUNTER.idFromString(named.toString().toUpperCase()); } catch { uppercaseRejected = true; }
      let placementRejected = false;
      try { env.COUNTER.getByName("alpha", { jurisdiction: "eu" }); } catch { placementRejected = true; }
      return Response.json({
        named: named.toString(),
        namedAgain: env.COUNTER.idFromName("alpha").toString(),
        unique: env.COUNTER.newUniqueId().toString(),
        mutatedIntrinsicNamed,
        crossNamespaceRejected,
        uppercaseRejected,
        placementRejected,
      });
    }
    const stub = env.COUNTER.getByName(name);
    if (url.pathname === "/committed-failure") {
      try { await stub.committedFailure(); } catch { return new Response("failed", { status: 503 }); }
      return new Response("unexpected success", { status: 500 });
    }
    if (url.pathname === "/held-write") {
      await stub.heldWrite();
      return new Response("released");
    }
    if (url.pathname === "/held-status") return Response.json(await stub.heldStatus());
    if (url.pathname === "/rpc") {
      const result = await stub.getValue();
      return new Response(`${result.release}:${result.value}`);
    }
    if (url.pathname === "/rpc-binary") {
      const result = await stub.echoBinary(new Uint8Array([4, 5, 6]));
      return new Response(Array.from(new Uint8Array(result)).join(","));
    }
    if (url.pathname === "/rollback") {
      const result = await stub.rollback();
      return new Response(`${result.rolledBack}:${result.value}`);
    }
    if (url.pathname === "/storage") {
      return Response.json(await stub.storageMatrix());
    }
    if (url.pathname === "/order") {
      await Promise.all([
        stub.ordered("first", 80),
        stub.ordered("second", 0),
      ]);
      return Response.json(await stub.orderValue());
    }
    if (url.pathname === "/websocket") {
      const response = await stub.fetch(new Request("https://object.invalid/?websocket=1", {
        headers: { Connection: "Upgrade", Upgrade: "websocket" },
      }));
      const socket = response.webSocket;
      if (!socket) return new Response("missing websocket", { status: 500 });
      socket.accept();
      const next = () => Promise.race([
        new Promise(resolve => socket.addEventListener("message", event => resolve(event.data), { once: true })),
        scheduler.wait(2000).then(() => { throw new Error("websocket timeout"); }),
      ]);
      socket.send("ping");
      const text = await next();
      socket.send(new Uint8Array([1, 2, 3]));
      const binary = await next();
      socket.close(1000, "done");
      let binaryBytes = binary instanceof ArrayBuffer
        ? new Uint8Array(binary)
        : ArrayBuffer.isView(binary) ? new Uint8Array(binary.buffer, binary.byteOffset, binary.byteLength) : null;
      if (!binaryBytes && binary instanceof Blob) binaryBytes = new Uint8Array(await binary.arrayBuffer());
      const binaryOk = Boolean(binaryBytes) && Array.from(binaryBytes).join(",") === "1,2,3";
      return new Response(`text:${text === "ping"},binary:${binaryOk}`);
    }
    const hold = url.pathname === "/hold" ? url.searchParams.get("ms") || "0" : "0";
    return stub.fetch(`https://object.invalid/?hold=${hold}`);
  }
};
