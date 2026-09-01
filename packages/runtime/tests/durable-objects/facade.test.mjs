import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

globalThis.scheduler = { wait: () => new Promise(() => {}) };

const codec = moduleUrl(await compileRuntime("durable-objects/id-codec.ts"));
const tunnel = moduleUrl(await compileRuntime("sockets/tunnel.ts"));
const cloudflare = moduleUrl(`
  export const connectCalls = [];
  export const operationStarts = [];
  export const background = [];
  export function waitUntil(promise) { background.push(Promise.resolve(promise)); }
`);
const { DurableObjectNamespace } = await importRuntime("durable-objects/facade.ts", {
  "cloudflare:workers": cloudflare,
  "../sockets/tunnel.js": tunnel,
  "./id-codec.js": codec,
});
const cloudflareModule = await import(cloudflare);

function namespace(prefix = "aaaaaaaaaaaaaaaa") {
  const calls = [];
  const prepared = new Map();
  const properties = {
    release: "A",
    "release-label": "A-label",
  };
  const transport = {
    dispatchRpc: (objectId, _channelId, _sequence, method, args) => {
      cloudflareModule.operationStarts.push("rpc:" + method);
      calls.push({ objectId, method, args });
      return nativeResult(Promise.resolve({ echoed: args, objectId, method }));
    },
    getRpcProperty: (_objectId, _channelId, _sequence, property) => {
      cloudflareModule.operationStarts.push("get:" + property);
      return nativeResult(Promise.resolve(properties[property]));
    },
    startRpc(objectId, channelId, sequence, kind, member, args) {
      let value;
      try {
        value = kind === "call"
          ? transport.dispatchRpc(objectId, channelId, sequence, member, args)
          : transport.getRpcProperty(objectId, channelId, sequence, member);
      } catch (error) {
        value = nativeResult(Promise.reject(error));
      }
      const holder = { [Symbol.dispose]() {} };
      const resolved = Promise.resolve(holder);
      return {
        then: resolved.then.bind(resolved),
        take() {
          const taken = value;
          value = undefined;
          return taken;
        },
      };
    },
    async cancelOrder() {},
    async prepareConnect(objectId, _channelId, _sequence, operationId, authority) {
      cloudflareModule.operationStarts.push("prepare-connect:" + authority.kind);
      prepared.set(operationId, { objectId, authority });
    },
    async cancelConnect(operationId) {
      cloudflareModule.operationStarts.push("cancel-connect:" + operationId);
      prepared.delete(operationId);
    },
    async dispatchFetch(_objectId, _channelId, _sequence, request) {
      cloudflareModule.operationStarts.push("fetch");
      return new Response("ok");
    },
    fetch(request) {
      const match = /^\/([0-9a-f]{64})\/([0-9a-f]{32})\/([0-9]+)$/.exec(new URL(request.url).pathname);
      return transport.dispatchFetch(match?.[1], match?.[2], Number(match?.[3]), request);
    },
    connect(tokenAddress, options) {
      cloudflareModule.operationStarts.push("connect");
      if (options?.secureTransport === "invalid") {
        throw new TypeError("native invalid secureTransport");
      }
      const operationId = /^([0-9a-f]{32})\.do-transport\.invalid:1$/.exec(tokenAddress)?.[1];
      const socket = { address: undefined, tokenAddress, options, opened: undefined, closed: undefined };
      const call = { objectId: undefined, address: undefined, options, socket };
      const opened = Promise.resolve().then(async () => {
        for (let attempt = 0; attempt < 20 && !prepared.has(operationId); attempt += 1) {
          await Promise.resolve();
        }
        const pending = operationId === undefined ? undefined : prepared.get(operationId);
        const address = pending?.authority.kind === "string"
          ? pending.authority.address
          : pending === undefined
            ? undefined
            : { hostname: pending.authority.hostname, port: pending.authority.port };
        socket.address = address;
        call.address = address;
        call.objectId = pending?.objectId;
        if (address === "failed.example:443" || pending === undefined) {
          throw new Error("native connect failed");
        }
        return {};
      });
      socket.opened = opened;
      socket.closed = opened.then(() => undefined);
      cloudflareModule.connectCalls.push(call);
      return socket;
    },
  };
  const ns = new DurableObjectNamespace({
    schemaVersion: 1,
    namespacePrefix: prefix,
    namespaceNameKey: Buffer.alloc(32).toString("base64"),
    maxObjectNameBytes: 64,
    transport,
  });
  return { ns, calls, transport };
}

function nativeResult(promise) {
  const target = () => undefined;
  return new Proxy(target, {
    get(_owner, property) {
      if (property === "then") return promise.then.bind(promise);
      const child = promise.then(value => Reflect.get(value, property, value));
      return nativeMember(promise, property, child);
    },
  });
}

function nativeMember(parent, property, child) {
  const target = (...args) => nativeResult(parent.then(value => (
    Reflect.apply(Reflect.get(value, property, value), value, args)
  )));
  return new Proxy(target, {
    get(_owner, nested) {
      if (nested === "then") return child.then.bind(child);
      return Reflect.get(nativeResult(child), nested);
    },
  });
}

function nativeStub(target, lifecycle = { disposed: 0, duplicated: 0 }) {
  return new Proxy(Object.create(null), {
    get(_owner, property) {
      if (property === "then") return undefined;
      if (property === "dup") return () => {
        lifecycle.duplicated += 1;
        return nativeStub(target, lifecycle);
      };
      if (property === Symbol.dispose) return () => { lifecycle.disposed += 1; };
      const value = Reflect.get(target, property, target);
      if (typeof value !== "function") return nativeResult(Promise.resolve(value));
      return (...args) => nativeResult(Promise.resolve().then(() => Reflect.apply(value, target, args)));
    },
  });
}

test("jurisdiction and placement options are accepted with stable local semantics", () => {
  const { ns } = namespace();
  const eu = ns.jurisdiction("eu");
  const named = eu.idFromName("alpha");
  assert.equal(named.jurisdiction, "eu");
  assert.equal(named.toString(), eu.idFromName("alpha").toString());
  assert.notEqual(named.toString(), ns.idFromName("alpha").toString());
  const unique = eu.newUniqueId({ jurisdiction: "eu" });
  assert.equal(unique.jurisdiction, "eu");
  assert.equal(ns.idFromString(unique.toString()).jurisdiction, "eu");
  const stub = eu.get(named, { locationHint: "enam", routingMode: "primary-only" });
  assert.equal(stub.id.toString(), named.toString());
  assert.equal(ns.get(named).id.toString(), named.toString());
  eu.getByName("alpha", { locationHint: "wnam" });
  ns.getByName("alpha", { locationHint: "wnam", extra: true });
  assert.equal(ns.jurisdiction(null).newUniqueId().jurisdiction, undefined);
  assert.equal(ns.newUniqueId({ jurisdiction: null, extra: true }).jurisdiction, undefined);
  assert.throws(() => ns.jurisdiction("mars"), /DO_ID_INVALID/);
  assert.throws(() => ns.getByName("alpha", { locationHint: "eu" }), /DO_ID_INVALID/);
  assert.throws(() => ns.getByName("alpha", { routingMode: "nearest" }), /DO_ID_INVALID/);
  assert.throws(() => ns.jurisdiction("us").get(unique), /DO_ID_INVALID/);
});

test("ID round-trip and namespace isolation stay exact", () => {
  const { ns } = namespace();
  const other = namespace("bbbbbbbbbbbbbbbb").ns;
  const named = ns.idFromName("alpha");
  const parsed = ns.idFromString(named.toString());
  assert.equal(parsed.toString(), named.toString());
  assert.equal(parsed.name, undefined);
  assert.equal(named.equals(parsed), true);
  assert.throws(() => other.idFromString(named.toString()), /DO_ID_INVALID/);
  assert.throws(() => ns.idFromString(named.toString().toUpperCase()), /DO_ID_INVALID/);
  const forged = `${named.toString().slice(0, -1)}${named.toString().endsWith("0") ? "1" : "0"}`;
  assert.throws(() => ns.idFromString(forged), /DO_ID_INVALID/);
});

test("RPC forwards native values and connect returns a native bridge Socket synchronously", async () => {
  const { ns, calls } = namespace();
  const stub = ns.getByName("rpc");
  const when = new Date("2026-08-30T00:00:00.000Z");
  const result = await stub.echo({ when, nested: new Map([["a", 1]]) });
  assert.deepEqual(calls[0].args[0].when, when);
  assert.equal(calls[0].args[0].nested.get("a"), 1);
  assert.equal(result.method, "echo");
  const socket = stub.connect("example.com:443", { allowHalfOpen: true });
  await socket.opened;
  assert.equal(socket.address, "example.com:443");
  assert.deepEqual(socket.options, { allowHalfOpen: true });
  assert.equal(cloudflareModule.connectCalls.at(-1).objectId, stub.id.toString());
  assert.match(socket.tokenAddress, /^[0-9a-f]{32}\.do-transport\.invalid:1$/);
  const ipv6 = { hostname: "2606:4700:4700::1111", port: 443 };
  const ipv6Socket = stub.connect(ipv6);
  await ipv6Socket.opened;
  assert.deepEqual(ipv6Socket.address, ipv6);
  assert.equal(cloudflareModule.operationStarts.at(-1), "prepare-connect:record");
});

test("connect preserves native option errors and cancels failed authorities", async () => {
  const { ns } = namespace();
  const stub = ns.getByName("connect-validation");
  const start = cloudflareModule.operationStarts.length;
  assert.throws(
    () => stub.connect("example.com:443", { secureTransport: "invalid" }),
    error => error instanceof TypeError && error.message === "native invalid secureTransport",
  );
  const asynchronousFailure = stub.connect("failed.example:443");
  await asynchronousFailure.opened.catch(() => undefined);
  const successful = stub.connect("ok.example:443");
  await successful.opened;
  await Promise.allSettled(cloudflareModule.background);
  const operations = cloudflareModule.operationStarts.slice(start);
  assert.equal(operations.filter(operation => operation.startsWith("prepare-connect:")).length, 3);
  assert.equal(operations.filter(operation => operation.startsWith("cancel-connect:")).length, 2);
  assert.equal(successful.address, "ok.example:443");
});

test("native clone failures are opaque and do not masquerade as unsupported RPC", async () => {
  const { ns, transport } = namespace();
  transport.dispatchRpc = () => nativeResult(Promise.reject(new TypeError("DataCloneError secret")));
  const stub = ns.getByName("rpc");
  let caught;
  try { await stub.echo(new WeakMap()); } catch (error) { caught = error; }
  assert.equal(caught?.message, "DO_RUNTIME_EXCEPTION");
  assert.equal(String(caught).includes("secret"), false);
});

test("dynamic properties, punctuation, and native promise pipelines stay intact", async () => {
  const { ns, transport } = namespace();
  const lifecycle = { disposed: 0, duplicated: 0 };
  const capability = nativeStub({
    label: "A-capability",
    echo(value) { return `A:${value}`; },
    fail() { throw new Error("tenant-capability-secret"); },
  }, lifecycle);
  const model = {
    release: "A",
    "release-label": "A-label",
    get failingProperty() { throw new Error("tenant-property-secret"); },
    "echo-value"(value) { return `A:${value}`; },
    capabilityValue() { return capability; },
    capabilityEnvelope() { return { target: capability }; },
    callbackValue(callback, value) { return callback(value); },
  };
  transport.dispatchRpc = (_objectId, _channelId, _sequence, method, args) => nativeResult(
    Promise.resolve().then(() => Reflect.apply(model[method], model, args)),
  );
  transport.getRpcProperty = (_objectId, _channelId, _sequence, property) => nativeResult(
    Promise.resolve().then(() => Reflect.get(model, property, model)),
  );
  const stub = ns.getByName("rpc");
  assert.equal(await stub.release, "A");
  assert.equal(await stub["release-label"], "A-label");
  assert.equal(await stub["echo-value"]("punctuation"), "A:punctuation");
  let caught;
  try { await stub.failingProperty; } catch (error) { caught = error; }
  assert.equal(caught?.message, "DO_RUNTIME_EXCEPTION");
  assert.equal(String(caught).includes("tenant-property-secret"), false);
  assert.equal(await stub.capabilityValue().echo("pipelined"), "A:pipelined");
  assert.equal(await stub.capabilityValue().label, "A-capability");
  assert.equal(await stub.callbackValue(value => `callback:${value}`, "ok"), "callback:ok");
  const held = await stub.capabilityValue();
  assert.equal(await held.echo("held"), "A:held");
  const duplicate = held.dup();
  assert.equal(await duplicate.echo("duplicate"), "A:duplicate");
  duplicate[Symbol.dispose]();
  held[Symbol.dispose]();
  assert.deepEqual(lifecycle, { disposed: 2, duplicated: 1 });
  caught = undefined;
  try { await held.fail(); } catch (error) { caught = error; }
  assert.equal(caught?.message, "DO_RUNTIME_EXCEPTION");
  assert.equal(String(caught).includes("tenant-capability-secret"), false);
  const envelope = await stub.capabilityEnvelope();
  caught = undefined;
  try { await envelope.target.fail(); } catch (error) { caught = error; }
  assert.equal(caught?.message, "DO_RUNTIME_EXCEPTION");
  assert.equal(String(caught).includes("tenant-capability-secret"), false);
});

test("the direct native transport preserves cross-surface start order without poisoning later calls", async () => {
  const { ns, transport } = namespace();
  const start = cloudflareModule.operationStarts.length;
  transport.dispatchRpc = (_objectId, _channelId, _sequence, method) => {
    cloudflareModule.operationStarts.push(`rpc:${method}`);
    return method === "first"
      ? nativeResult(Promise.reject(new Error("first failed")))
      : nativeResult(Promise.resolve(method));
  };
  transport.dispatchFetch = async () => {
    cloudflareModule.operationStarts.push("fetch");
    return new Response("fetch-ok");
  };
  const stub = ns.getByName("rpc");
  const first = Promise.resolve(stub.first()).then(
    () => false,
    error => error?.message === "DO_RUNTIME_EXCEPTION",
  );
  const fetched = stub.fetch("https://object.invalid/");
  const socket = stub.connect("example.com:443");
  const second = stub.second();
  assert.equal(await first, true);
  assert.equal(await (await fetched).text(), "fetch-ok");
  await socket.opened;
  assert.equal(socket.address, "example.com:443");
  assert.equal(await second, "second");
  assert.deepEqual(cloudflareModule.operationStarts.slice(start), [
    "rpc:first", "connect", "fetch", "prepare-connect:string", "rpc:second",
  ]);
});

test("stock RPC serializable values are forwarded without a local allowlist", async () => {
  const { ns, transport } = namespace();
  transport.dispatchRpc = (_objectId, _channelId, _sequence, _method, args) =>
    nativeResult(Promise.resolve(args[0]));
  const stub = ns.getByName("rpc");
  const error = new TypeError("returned value");
  const input = {
    bigint: 12n,
    map: new Map([["key", new Set([1, 2])]]),
    regexp: /native/giu,
    error,
    data: new DataView(Uint8Array.from([3, 4]).buffer),
    headers: new Headers({ "x-value": "ok" }),
  };
  const output = await stub.echo(input);
  assert.equal(output.bigint, 12n);
  assert.deepEqual([...output.map.get("key")], [1, 2]);
  assert.equal(output.regexp.source, "native");
  assert.equal(output.error, error);
  assert.equal(output.data.getUint8(1), 4);
  assert.equal(output.headers.get("x-value"), "ok");
});

test("reserved Durable Object handlers never become RPC methods", () => {
  const { ns } = namespace();
  const stub = ns.getByName("rpc");
  for (const method of ["dup", "alarm", "webSocketMessage", "webSocketClose", "webSocketError"]) {
    assert.throws(() => stub[method], /DO_RPC_UNSUPPORTED/);
  }
});
