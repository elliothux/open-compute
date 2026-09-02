import { DurableObject, RpcTarget, exports as importableExports } from "cloudflare:workers";

class EchoCapability extends RpcTarget {
  constructor(release) {
    super();
    this.release = release;
  }
  get label() { return `${this.release}:capability`; }
  get nested() { return new NestedCapability(this.release); }
  echo(value) { return `${this.release}:${value}`; }
  fail() { throw new Error("tenant-capability-secret"); }
}

class NestedCapability extends RpcTarget {
  constructor(release) {
    super();
    this.release = release;
  }
  echo(value) { return `${this.release}:nested:${value}`; }
}

class CallerCapability extends RpcTarget {
  echo(value) { return `target:${value}`; }
}

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
    const constructs = (this.ctx.storage.kv.get("constructs") || 0) + 1;
    this.ctx.storage.kv.put("constructs", constructs);
    this.constructs = constructs;
  }
  async fetch(request) {
    const url = new URL(request.url);
    for (const name of [
      "x-open-compute-binding-token",
      "x-open-compute-account-id",
      "x-open-compute-worker-id",
      "x-open-compute-binding-id",
      "x-open-compute-version-id",
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
    if (url.searchParams.get("hibernate") === "1") {
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      this.ctx.acceptWebSocket(server, ["echo"]);
      server.serializeAttachment({ n: 1, tag: "echo" });
      this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
      return new Response(null, { status: 101, webSocket: client });
    }
    const orderLabel = url.searchParams.get("order");
    if (orderLabel) {
      await this.ordered(orderLabel, Number(url.searchParams.get("hold") || 0));
      return new Response(orderLabel);
    }
    const hold = Number(url.searchParams.get("hold") || 0);
    let holdWindow = null;
    if (hold > 0) {
      this.ctx.storage.kv.put("fetch-hold-started", true);
      const t0 = Date.now();
      await scheduler.wait(hold);
      holdWindow = { t0, t1: Date.now() };
    }
    const value = this.ctx.storage.transactionSync(() => increment(this.ctx.storage.sql));
    await this.ctx.storage.sync();
    if (holdWindow) {
      return Response.json({ ...holdWindow, value: `${this.env.RELEASE}:${value}` });
    }
    return new Response(`${this.env.RELEASE}:${value}`);
  }
  async connect(socket) {
    await this.ordered("connect", 0);
    const reader = socket.readable.getReader();
    const writer = socket.writable.getWriter();
    const part = await reader.read();
    if (!part.done) await writer.write(part.value);
    await writer.close();
    writer.releaseLock();
    await reader.cancel();
    reader.releaseLock();
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
  get releaseLabel() { return `${this.env.RELEASE}:property`; }
  get ["release-label"]() { return `${this.env.RELEASE}:punctuation`; }
  get failingProperty() { throw new Error("tenant-property-secret"); }
  ["echo-value"](value) { return `${this.env.RELEASE}:${value}`; }
  async echoBinary(value) { return value; }
  async echoValue(value) { return value; }
  streamValue() {
    return new ReadableStream({
      start(controller) {
        controller.enqueue(new Uint8Array([7, 8]));
        controller.enqueue(new Uint8Array([9]));
        controller.close();
      },
    });
  }
  writableValue() {
    this.written = [];
    return new WritableStream({
      write: chunk => { this.written.push(...new Uint8Array(chunk)); },
    });
  }
  writtenValue() { return this.written || []; }
  capabilityValue() { return new EchoCapability(this.env.RELEASE); }
  capabilityEnvelope() { return { target: new EchoCapability(this.env.RELEASE) }; }
  async delayedCapability(ms) {
    this.ctx.storage.kv.put("capability-hold-started", true);
    await scheduler.wait(ms);
    return new EchoCapability(this.env.RELEASE);
  }
  holdStarted() {
    return {
      fetch: this.ctx.storage.kv.get("fetch-hold-started") === true,
      capability: this.ctx.storage.kv.get("capability-hold-started") === true,
    };
  }
  callTarget(target, value) { return target.echo(value); }
  callFunction(callback, value) { return callback(value); }
  rpcFailure() { throw new Error("tenant-rpc-secret"); }
  async webSocketMessage(ws, message) {
    if (message === "boom") throw new Error("fixture-ws-error");
    const attachment = ws.deserializeAttachment();
    this.ctx.storage.kv.put("ws-message", attachment === null ? null : attachment);
    ws.send(message);
  }
  async webSocketClose(_ws, code, reason, wasClean) {
    this.ctx.storage.kv.put("ws-close", { code, reason, wasClean });
  }
  async webSocketError(_ws, error) {
    this.ctx.storage.kv.put("ws-error", String(error));
  }
  async triggerAbort() {
    this.ctx.abort("fixture-abort");
    return false;
  }
  async commitOutput() {
    await this.ctx.storage.transaction(async txn => {
      await txn.put("output", true);
      this.env.EVENTS.send({ kind: "do-output" }).catch(() => undefined);
    });
    return { stored: await this.ctx.storage.get("output") === true };
  }
  async rollbackOutput() {
    await this.ctx.storage.transaction(async txn => {
      await txn.put("rolled-output", true);
      await this.env.EVENTS.send({ kind: "do-output-rollback" });
      txn.rollback();
    });
    return {
      stored: await this.ctx.storage.get("rolled-output") === true,
      metrics: await this.env.EVENTS.metrics(),
    };
  }
  async failedOutput() {
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("failed-output", true);
        await this.env.EVENTS.send({ kind: "do-output-failure" });
        throw new Error("fixture-output-transaction-failed");
      });
    } catch {}
    return {
      stored: await this.ctx.storage.get("failed-output") === true,
      metrics: await this.env.EVENTS.metrics(),
    };
  }
  async outputMetrics() {
    return this.env.EVENTS.metrics();
  }
  async hibernateInspect() {
    const sockets = this.ctx.getWebSockets("echo");
    const first = sockets[0];
    const pair = this.ctx.getWebSocketAutoResponse();
    return {
      constructs: this.ctx.storage.kv.get("constructs") || 0,
      instanceConstructs: this.constructs,
      sockets: sockets.length,
      tags: first ? this.ctx.getTags(first) : [],
      attachment: first ? first.deserializeAttachment() : null,
      autoRequest: pair ? pair.request : null,
      autoResponse: pair ? pair.response : null,
      autoTimestamp: first
        ? this.ctx.getWebSocketAutoResponseTimestamp(first) instanceof Date
          || this.ctx.getWebSocketAutoResponseTimestamp(first) === null
        : true,
      timeout: this.ctx.getHibernatableWebSocketEventTimeout(),
      close: this.ctx.storage.kv.get("ws-close") || null,
      error: this.ctx.storage.kv.get("ws-error") || null,
    };
  }
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
      const listed = [...this.ctx.storage.kv.list({ prefix: "s" })].some(([key]) => key === "sync");
      result.syncKv = value.value === 1 && listed && this.ctx.storage.kv.delete("sync");
    } catch { return { failedStage: "syncKv" }; }
    try {
      await this.ctx.storage.put("async", { value: 2 }, { allowConcurrency: true, noCache: true });
      const value = await this.ctx.storage.get("async", { allowConcurrency: true, noCache: true });
      const bulk = await this.ctx.storage.get(["async"], { allowConcurrency: true });
      const listed = (await this.ctx.storage.list()).has("async");
      result.asyncKv = value.value === 2 && bulk.get("async").value === 2 && listed
        && await this.ctx.storage.delete("async", { allowConcurrency: true });
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
      await this.ctx.storage.put("explicit-rollback", 1);
      await this.ctx.storage.transaction(async txn => {
        await txn.put("explicit-rollback", 2);
        txn.rollback();
      });
      result.transactionRollback = await this.ctx.storage.get("explicit-rollback") === 1;
      await this.ctx.storage.delete("explicit-rollback");
    } catch { return { failedStage: "transactionRollback" }; }
    try {
      result.transactionSync = this.ctx.storage.transactionSync(() => {
        this.ctx.storage.kv.put("sync-txn", 1);
        return this.ctx.storage.kv.get("sync-txn") === 1;
      }) === true && this.ctx.storage.kv.delete("sync-txn");
    } catch { return { failedStage: "transactionSync" }; }
    try {
      await this.ctx.storage.put({ "list/b": 2, "list/a": 1, "list/c": 3, "other": 9 });
      const listed = [...(await this.ctx.storage.list({
        prefix: "list/", reverse: true, limit: 2, start: "list/", end: "list/z",
      })).keys()];
      const after = [...(await this.ctx.storage.list({ prefix: "list/", startAfter: "list/a" })).keys()];
      result.listOptions = listed[0] === "list/c" && listed[1] === "list/b" && after.includes("list/b");
      await this.ctx.storage.delete(["list/a", "list/b", "list/c", "other"]);
    } catch { return { failedStage: "listOptions" }; }
    try {
      this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS cursor_probe(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
      this.ctx.storage.sql.exec("INSERT INTO cursor_probe(id, value) VALUES (1, 7)");
      const cursor = this.ctx.storage.sql.exec("SELECT id, value FROM cursor_probe WHERE id = ?", 1);
      const one = cursor.one();
      const again = this.ctx.storage.sql.exec("SELECT id, value FROM cursor_probe");
      const names = again.columnNames.slice();
      const raw = [...again.raw()];
      const next = this.ctx.storage.sql.exec("SELECT value FROM cursor_probe").next();
      result.sqlCursor = Number(one.value) === 7 && names.includes("value")
        && raw[0][1] === 7 && next.done === false && again.rowsRead >= 1
        && this.ctx.storage.sql.databaseSize > 0
        && typeof this.ctx.storage.sql.Cursor === "function"
        && typeof this.ctx.storage.sql.Statement === "function";
      this.ctx.storage.sql.exec("DROP TABLE cursor_probe");
    } catch { return { failedStage: "sqlCursor" }; }
    try {
      await this.ctx.storage.sync();
      result.sync = true;
    } catch { return { failedStage: "sync" }; }
    try {
      const current = await this.ctx.storage.getCurrentBookmark();
      const next = await this.ctx.storage.getCurrentBookmark();
      result.bookmarks = /^[0-9a-f]{8}-[0-9a-f]{8}-[0-9a-f]{8}-[0-9a-f]{32}$/.test(current)
        && next > current;
      const unsupported = async operation => {
        try { await operation(); }
        catch (error) {
          return String(error).includes("does not implement point-in-time recovery");
        }
        return false;
      };
      result.pitrUnsupported = await unsupported(
        () => this.ctx.storage.getBookmarkForTime(new Date()),
      ) && await unsupported(
        () => this.ctx.storage.onNextSessionRestoreBookmark(current),
      );
    } catch { return { failedStage: "bookmarks" }; }
    try {
      const due = Date.now() + 60_000;
      await this.ctx.storage.setAlarm(due, { allowConcurrency: true });
      const read = await this.ctx.storage.getAlarm({ allowConcurrency: true });
      await this.ctx.storage.deleteAlarm({ allowConcurrency: true });
      result.alarms = read === due && await this.ctx.storage.getAlarm() === null;
    } catch { return { failedStage: "alarms" }; }
    try {
      const exports = this.ctx.exports;
      const privateNames = Reflect.ownKeys(exports)
        .filter(key => typeof key === "string" && key.startsWith("__OpenCompute"));
      result.exports = typeof exports?.Counter === "function";
      result.privateExportsHidden = privateNames.length === 0
        && !Reflect.ownKeys(importableExports).some(
          key => typeof key === "string" && key.startsWith("__OpenCompute"),
        );
      result.props = this.ctx.props !== null && typeof this.ctx.props === "object"
        && Reflect.ownKeys(this.ctx.props).length === 0;
      result.id = typeof this.ctx.id?.toString() === "string";
      result.facets = this.ctx.facets !== undefined;
      result.containerAbsent = this.ctx.container === undefined;
    } catch { return { failedStage: "context" }; }
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
      await this.ctx.storage.setAlarm(Date.now() + 30_000);
      await this.ctx.storage.deleteAll({ allowUnconfirmed: false });
      result.deleteAll = this.ctx.storage.kv.get("delete-all") === undefined
        && await this.ctx.storage.get("delete-all") === undefined
        && await this.ctx.storage.getAlarm() === null;
      this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS counter(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)");
    } catch (error) { return { failedStage: "deleteAll", detail: String(error) }; }
    return result;
  }
  async facetMatrix() {
    const childClass = this.ctx.exports.FacetCounter({ props: { marker: "facet" } });
    const child = this.ctx.facets.get("child", () => ({ class: childClass, id: "facet-id" }));
    const first = await child.increment();
    const same = this.ctx.facets.get("child", () => { throw new Error("unexpected facet restart"); });
    const second = await same.increment();
    const props = await child.readProps();
    const id = await child.readId();
    this.ctx.facets.clone("child", "copy");
    const copy = this.ctx.facets.get("copy", () => ({ class: childClass, id: "copy-id" }));
    const cloned = await copy.increment();
    this.ctx.facets.delete("copy");
    this.ctx.facets.delete("never-created");
    const freshCopy = this.ctx.facets.get("copy", () => ({ class: childClass, id: "fresh-copy" }));
    const fresh = await freshCopy.increment();
    this.ctx.facets.abort("child", new Error("facet-aborted"));
    let aborted = false;
    try { await child.increment(); } catch (error) { aborted = String(error).includes("facet-aborted"); }
    const recovered = this.ctx.facets.get("child", () => ({ class: childClass, id: "facet-id" }));
    const afterAbort = await recovered.increment();
    return {
      first,
      second,
      props,
      id,
      cloned,
      fresh,
      aborted,
      afterAbort,
    };
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

export class FacetCounter extends DurableObject {
  async increment() {
    const value = (await this.ctx.storage.get("value")) || 0;
    await this.ctx.storage.put("value", value + 1);
    return value + 1;
  }
  readProps() { return this.ctx.props; }
  readId() { return this.ctx.id.toString(); }
}

export class OtherCounter extends Counter {}

export default {
  async fetch(request, env, ctx) {
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
      let invalidHintRejected = false;
      try { env.COUNTER.getByName("alpha", { locationHint: "eu" }); } catch { invalidHintRejected = true; }
      const eu = env.COUNTER.jurisdiction("eu");
      const scoped = eu.newUniqueId({ jurisdiction: "eu" });
      const scopedNamed = eu.idFromName("alpha");
      const parsedScoped = env.COUNTER.idFromString(scopedNamed.toString());
      let forgedRejected = false;
      const forged = `${scopedNamed.toString().slice(0, -1)}${scopedNamed.toString().endsWith("0") ? "1" : "0"}`;
      try { env.COUNTER.idFromString(forged); } catch { forgedRejected = true; }
      let locationAccepted = true;
      try { env.COUNTER.getByName("alpha", { locationHint: "enam", routingMode: "primary-only" }); }
      catch { locationAccepted = false; }
      return Response.json({
        named: named.toString(),
        namedAgain: env.COUNTER.idFromName("alpha").toString(),
        unique: env.COUNTER.newUniqueId().toString(),
        mutatedIntrinsicNamed,
        crossNamespaceRejected,
        uppercaseRejected,
        invalidHintRejected,
        locationAccepted,
        jurisdiction: scoped.jurisdiction,
        namedJurisdiction: scopedNamed.jurisdiction,
        jurisdictionRoundTrip: parsedScoped.jurisdiction === "eu" && parsedScoped.name === undefined,
        jurisdictionChangesId: scopedNamed.toString() !== named.toString(),
        unscopedGetAcceptsJurisdiction: env.COUNTER.get(scopedNamed).id.equals(scopedNamed),
        nullishJurisdiction: env.COUNTER.jurisdiction(null).newUniqueId().jurisdiction === undefined
          && env.COUNTER.newUniqueId({ jurisdiction: null }).jurisdiction === undefined,
        forgedRejected,
        forgedBridgeRejected: true,
      });
    }
    const stub = env.COUNTER.getByName(name);
    if (url.pathname === "/connect" || url.pathname === "/connect-ipv6") {
      const ipv6 = url.pathname === "/connect-ipv6";
      const socket = stub.connect(
        ipv6 ? { hostname: "2606:4700:4700::1111", port: 7000 } : "counter.invalid:7000",
        { allowHalfOpen: true },
      );
      await socket.opened;
      try {
        const writer = socket.writable.getWriter();
        await writer.write(new Uint8Array(ipv6 ? [10, 11, 12] : [4, 5, 6]));
        await writer.close();
        writer.releaseLock();
        const bytes = new Uint8Array(await new Response(socket.readable).arrayBuffer());
        return new Response(Array.from(bytes).join(","));
      } finally {
        await socket.close().catch(() => undefined);
      }
    }
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
    if (url.pathname === "/rpc-structured") {
      const result = await stub.echoValue({
        bigint: 12n,
        when: new Date("2026-08-30T00:00:00.000Z"),
        map: new Map([["key", new Set([1, 2])]]),
        regexp: /native/giu,
        error: new TypeError("returned-error-value"),
        typed: new Uint16Array([3, 4]),
        view: new DataView(Uint8Array.from([5, 6]).buffer),
        buffer: Uint8Array.from([7, 8]).buffer,
        headers: new Headers({ "x-rpc": "ok" }),
        request: new Request("https://rpc.invalid/request", { method: "POST", body: "request-body" }),
        response: new Response("response-body", { headers: { "x-response": "ok" } }),
      });
      return Response.json({
        time: result.when instanceof Date ? result.when.toISOString() : String(result.when),
        bigint: result.bigint === 12n,
        map: result.map instanceof Map && result.map.get("key") instanceof Set
          && [...result.map.get("key")].join(",") === "1,2",
        regexp: result.regexp instanceof RegExp && result.regexp.flags === "giu",
        error: result.error instanceof TypeError && result.error.message === "returned-error-value",
        typed: result.typed instanceof Uint16Array && result.typed.join(",") === "3,4",
        view: result.view instanceof DataView && result.view.getUint8(1) === 6,
        buffer: result.buffer instanceof ArrayBuffer
          && new Uint8Array(result.buffer).join(",") === "7,8",
        headers: result.headers instanceof Headers && result.headers.get("x-rpc") === "ok",
        request: result.request instanceof Request && await result.request.text() === "request-body",
        response: result.response instanceof Response
          && result.response.headers.get("x-response") === "ok"
          && await result.response.text() === "response-body",
      });
    }
    if (url.pathname === "/rpc-stream") {
      const stream = await stub.streamValue();
      const bytes = new Uint8Array(await new Response(stream).arrayBuffer());
      return new Response(Array.from(bytes).join(","));
    }
    if (url.pathname === "/rpc-writable") {
      const stream = await stub.writableValue();
      const writer = stream.getWriter();
      await writer.write(new Uint8Array([10, 11]));
      await writer.close();
      return new Response((await stub.writtenValue()).join(","));
    }
    if (url.pathname === "/rpc-capability") {
      return Response.json({
        direct: await stub.capabilityValue().echo("ok"),
        property: await stub.capabilityValue().label,
        nested: await stub.capabilityValue().nested.echo("ok"),
        envelope: await stub.capabilityEnvelope().target.echo("ok"),
      });
    }
    if (url.pathname === "/rpc-pipeline-hold") {
      return new Response(await stub.delayedCapability(
        Number(url.searchParams.get("ms") || 0),
      ).echo("ok"));
    }
    if (url.pathname === "/hold-started") return Response.json(await stub.holdStarted());
    if (url.pathname === "/rpc-property") {
      return Response.json({
        regular: await stub.releaseLabel,
        punctuation: await stub["release-label"],
        method: await stub["echo-value"]("punctuation-method"),
      });
    }
    if (url.pathname === "/rpc-property-error") {
      try { await stub.failingProperty; }
      catch (error) {
        const message = String(error);
        return new Response(message.includes("DO_RUNTIME_EXCEPTION")
          && !message.includes("tenant-property-secret") ? "true" : message);
      }
      return new Response("unexpected success", { status: 500 });
    }
    if (url.pathname === "/rpc-callback") {
      return Response.json({
        target: await stub.callTarget(new CallerCapability(), "ok"),
        callback: await stub.callFunction(value => `function:${value}`, "ok"),
      });
    }
    if (url.pathname === "/rpc-clone-error") {
      try { await stub.echoValue(new WeakMap()); }
      catch (error) {
        const message = String(error);
        return new Response(message.includes("DO_RUNTIME_EXCEPTION")
          && !message.includes("WeakMap") && !message.includes("clone") ? "true" : message);
      }
      return new Response("unexpected success", { status: 500 });
    }
    if (url.pathname === "/rpc-capability-error") {
      const capability = await stub.capabilityValue();
      try { await capability.fail(); }
      catch (error) {
        const message = String(error);
        return new Response(!message.includes("tenant-capability-secret") ? "true" : message);
      }
      return new Response("unexpected success", { status: 500 });
    }
    if (url.pathname === "/rpc-error") {
      try { await stub.rpcFailure(); }
      catch (error) {
        const message = String(error);
        return new Response(message.includes("DO_RUNTIME_EXCEPTION") && !message.includes("tenant-rpc-secret") ? "true" : message);
      }
      return new Response("unexpected success", { status: 500 });
    }
    if (url.pathname === "/rollback") {
      const result = await stub.rollback();
      return new Response(`${result.rolledBack}:${result.value}`);
    }
    if (url.pathname === "/storage") {
      return Response.json(await stub.storageMatrix());
    }
    if (url.pathname === "/facets") {
      return Response.json(await stub.facetMatrix());
    }
    if (url.pathname === "/order") {
      await Promise.all([
        stub.ordered("first", 80),
        stub.ordered("second", 0),
      ]);
      return Response.json(await stub.orderValue());
    }
    if (url.pathname === "/cross-order") {
      const first = stub.ordered("rpc-first", 80);
      const fetched = stub.fetch("https://object.invalid/?order=fetch-second");
      const socket = stub.connect("counter.invalid:7000", { allowHalfOpen: true });
      const fourth = stub.ordered("rpc-fourth", 0);
      await socket.opened;
      const writer = socket.writable.getWriter();
      await writer.write(new Uint8Array([7]));
      await writer.close();
      writer.releaseLock();
      const echoed = new Uint8Array(await new Response(socket.readable).arrayBuffer());
      await socket.close().catch(() => undefined);
      await Promise.all([first, fetched, fourth]);
      return Response.json({ order: await stub.orderValue(), echoed: echoed[0] === 7 });
    }
    if (url.pathname === "/order-error") {
      const failed = stub.rpcFailure().then(
        () => false,
        error => String(error).includes("DO_RUNTIME_EXCEPTION"),
      );
      const fetched = stub.fetch("https://object.invalid/?order=fetch-after-error");
      const rpc = stub.ordered("rpc-after-error", 0);
      return Response.json({
        failed: await failed,
        fetched: await (await fetched).text() === "fetch-after-error",
        rpc: Array.isArray(await rpc),
      });
    }
    if (url.pathname === "/hibernate") {
      const response = await stub.fetch(new Request("https://object.invalid/?hibernate=1", {
        headers: { Connection: "Upgrade", Upgrade: "websocket" },
      }));
      const socket = response.webSocket;
      if (!socket) return new Response("missing hibernate websocket", { status: 500 });
      socket.accept();
      const next = () => Promise.race([
        new Promise(resolve => socket.addEventListener("message", event => resolve(event.data), { once: true })),
        scheduler.wait(2000).then(() => { throw new Error("hibernate timeout"); }),
      ]);
      socket.send("ping");
      const auto = await next();
      const before = await stub.hibernateInspect();
      socket.send("hello");
      const echoed = await next();
      socket.close(1000, "done");
      await scheduler.wait(50);
      const afterClose = await stub.hibernateInspect();
      return Response.json({
        auto: auto === "pong",
        echoed: echoed === "hello",
        sockets: before.sockets === 1,
        tags: Array.isArray(before.tags) && before.tags.includes("echo"),
        attachment: before.attachment?.n === 1 && before.attachment?.tag === "echo",
        autoRequest: before.autoRequest === "ping",
        autoResponse: before.autoResponse === "pong",
        closed: afterClose.close?.code === 1000,
      });
    }
    if (url.pathname === "/commit-output") {
      await stub.commitOutput();
      return Response.json(await stub.outputMetrics());
    }
    if (url.pathname === "/rollback-output") return Response.json(await stub.rollbackOutput());
    if (url.pathname === "/failed-output") return Response.json(await stub.failedOutput());
    if (url.pathname === "/output-metrics") return Response.json(await stub.outputMetrics());
    if (url.pathname === "/hibernate-open") {
      const response = await stub.fetch(new Request("https://object.invalid/?hibernate=1", {
        headers: { Connection: "Upgrade", Upgrade: "websocket" },
      }));
      const socket = response.webSocket;
      if (!socket) return new Response("missing hibernate websocket", { status: 500 });
      socket.accept();
      return Response.json(await stub.hibernateInspect());
    }
    if (url.pathname === "/hibernate-inspect") return Response.json(await stub.hibernateInspect());
    if (url.pathname === "/abort") {
      let aborted = false;
      try { await stub.triggerAbort(); } catch { aborted = true; }
      const recovered = await stub.getValue();
      return Response.json({ aborted, recovered: typeof recovered.value === "number" });
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
    const response = await stub.fetch(`https://object.invalid/?hold=${hold}`);
    if (url.pathname === "/hold" && hold !== "0" && url.searchParams.get("window") !== "1") {
      const payload = await response.json();
      return new Response(payload.value, { status: response.status });
    }
    return response;
  }
};
