import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

globalThis.scheduler = { wait: () => new Promise(() => {}) };

const cloudflare = moduleUrl(`
  export class RpcTarget {}
  export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }
  export let env = {};
  export const connectCalls = [];
  export const exports = {
    __OpenComputeServiceConnectTransport({ props }) {
      return { connect(address, options) {
        const socket = { address, options };
        connectCalls.push({ props, address, options, socket });
        return socket;
      } };
    },
  };
  export const background = [];
  export function waitUntil(promise) { background.push(Promise.resolve(promise)); }
  export function withEnv(next, action) {
    const previous = env;
    env = next;
    try {
      const result = action();
      if (result && typeof result.then === "function") {
        return Promise.resolve(result).finally(() => { env = previous; });
      }
      env = previous;
      return result;
    } catch (error) { env = previous; throw error; }
  }
`);
const scopeUrl = moduleUrl(await compileRuntime("services/scope.ts", {
  "cloudflare:workers": cloudflare,
}));
const facadeUrl = moduleUrl(await compileRuntime("services/facade.ts", {
  "cloudflare:workers": cloudflare,
  "./scope.js": scopeUrl,
}));
const cloudflareModule = await import(cloudflare);
const { ServiceBinding, completeServiceScope, decodeServiceValue, encodeServiceValue } = await import(facadeUrl);
const { rootServiceFrame, withServiceScope } = await import(scopeUrl);

function activation(events) {
  return {
    async begin() {
      const handle = crypto.randomUUID();
      events.push(["begin", handle]);
      return { handle, frame: crypto.randomUUID(), deadlineMs: 30_000 };
    },
    async complete(handle) { events.push(["complete", handle]); },
    async release() { events.push(["release"]); },
  };
}

test("Service facade preserves methods, getters, callbacks, returned targets, and root completion", async () => {
  const events = [];
  const raw = {
    connect(address, options) {
      if (address === "malformed") throw new TypeError("native malformed address");
      const opened = address === "failed.example:443"
        ? Promise.reject(new Error("native connect failed"))
        : Promise.resolve({});
      const socket = { address, options, opened };
      events.push(["connect", address, options]);
      return socket;
    },
    async rpc(_frame, method, args) {
      if (method === "add") return args[0] + args[1];
      if (method === "callback") {
        args[0].handle.activate(activation(events));
        const [callback] = decodeServiceValue(args);
        const value = await callback(41);
        callback[Symbol.dispose]();
        return value;
      }
      if (method === "target") {
        class Counter extends cloudflareModule.RpcTarget {
          get version() { return 7; }
          increment(value) { return value + 1; }
        }
        const envelope = encodeServiceValue(new Counter(), raw);
        envelope.handle.activate(activation(events));
        return envelope;
      }
      throw new Error("unexpected method");
    },
    async get(_frame, property) {
      if (property === "version") return 3;
      throw new Error("unexpected property");
    },
    async fetchService(_frame, request) { return new Response(request.url); },
    async completeRoot(scopeId) { events.push(["root", scopeId]); },
    async beginCapability() { throw new Error("not used"); },
    async releaseRetention() {},
    async completeOperation() {},
  };
  const service = new ServiceBinding(raw, false, "TARGET");
  assert.equal(service.then, undefined);
  assert.throws(() => service.constructor, /SERVICE_BINDING_DENIED/);
  const frame = rootServiceFrame();
  await withServiceScope({ SERVICE: service }, frame, async scoped => {
    assert.equal(await service.add(1, 2), 3);
    assert.equal(await service.version, 3);
    assert.equal(await service.callback(value => value + 1), 42);
    const target = await service.target();
    assert.equal(target.then, undefined);
    assert.throws(() => target.constructor, /SERVICE_BINDING_DENIED/);
    assert.equal(await target.increment(9), 10);
    assert.equal(await target.version, 7);
    target[Symbol.dispose]();
    assert.equal(await (await service.fetch("https://example.invalid/path")).text(), "https://example.invalid/path");
    const socket = service.connect("example.com:443", { allowHalfOpen: true });
    assert.equal(socket.address, "example.com:443");
    assert.deepEqual(socket.options, { allowHalfOpen: true });
    assert.deepEqual(events.find(event => event[0] === "connect"), [
      "connect", "example.com:443", { allowHalfOpen: true },
    ]);
    const ipv6 = { hostname: "2606:4700:4700::1111", port: 443 };
    const ipv6Socket = service.connect(ipv6);
    assert.equal(ipv6Socket.address, ipv6);
    await completeServiceScope(scoped, frame.scopeId);
  });
  await Promise.allSettled(cloudflareModule.background);
  assert.equal(events.filter(event => event[0] === "begin").length, 3);
  assert.equal(events.filter(event => event[0] === "complete").length, 3);
  assert.equal(events.filter(event => event[0] === "release").length, 2);
  assert.deepEqual(events.at(-1), ["root", frame.scopeId]);
});

test("Service connect preserves native address forms and errors", async () => {
  const events = [];
  const raw = {
    connect(address) {
      events.push(["connect", address]);
      if (address === "malformed") throw new TypeError("native malformed address");
      return {
        address,
        opened: address === "failed.example:443"
          ? Promise.reject(new Error("native connect failed"))
          : Promise.resolve({}),
      };
    },
    async fetchService() { throw new Error("not used"); },
    async rpc() { throw new Error("not used"); },
    async get() { throw new Error("not used"); },
  };
  const service = new ServiceBinding(raw, false, "TARGET");

  assert.throws(
    () => service.connect("malformed"),
    error => error instanceof TypeError && error.message === "native malformed address",
  );
  const failed = service.connect("failed.example:443");
  await failed.opened.catch(() => undefined);
  const successful = service.connect("ok.example:443");
  await successful.opened;
  assert.deepEqual(events.filter(event => event[0] === "connect").map(event => event[1]), [
    "malformed",
    "failed.example:443",
    "ok.example:443",
  ]);
});
