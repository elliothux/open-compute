import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";

function assert(value, message) {
  if (!value) throw new Error(message);
}

function isObject(value) {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function isPlain(value) {
  return Array.isArray(value) || Object.getPrototypeOf(value) === Object.prototype
    || Object.getPrototypeOf(value) === null;
}

let targetDisposals = 0;
let retentionBegins = 0;
let retentionCompletes = 0;
let retentionReleases = 0;
let writableText = "";

class Target extends RpcTarget {
  constructor(value) {
    super();
    this.value = value;
  }

  get label() { return `label:${this.value}`; }
  ping(suffix) { return `${this.value}:${suffix}`; }
  nested() { return new Target(`${this.value}:nested`); }
  [Symbol.dispose]() { targetDisposals += 1; }
}

class BackendLogic {
  get version() { return "backend-v1"; }
  scalar(value) { return value + 1; }
  structured(value) {
    return { value, bytes: new Uint8Array([1, 2, 3]), when: new Date(1_700_000_000_000) };
  }
  request(request) { return { url: request.url, method: request.method }; }
  response() { return new Response("native-response", { status: 201 }); }
  readable() {
    return new ReadableStream({
      start(controller) { controller.enqueue(new TextEncoder().encode("stream")); controller.close(); },
    });
  }
  writable() {
    return new WritableStream({
      write(chunk) { writableText += new TextDecoder().decode(chunk); },
    });
  }
  written() { return writableText; }
  capability(value) { return new Target(value); }
  callback(callback, value) { return callback(value); }
  targetCallback(callback, value) { return callback.ping(value); }
  invalid() { return new WeakMap(); }
  disposalCount() { return targetDisposals; }
  fetch(request) {
    if (new URL(request.url).pathname === "/websocket") {
      const pair = new WebSocketPair();
      pair[1].accept();
      pair[1].addEventListener("message", event => pair[1].send(`echo:${event.data}`));
      return new Response(null, { status: 101, webSocket: pair[0] });
    }
    return new Response(`${request.method}:${new URL(request.url).hostname}`);
  }
}

const backend = new BackendLogic();

export class Backend extends WorkerEntrypoint {
  async invoke(method, rawArgs, controller) {
    const args = decode(rawArgs);
    return encode(await Reflect.apply(backend[method], backend, args), controller);
  }

  async getProperty(property, controller) {
    return encode(await backend[property], controller);
  }

  fetch(request) { return backend.fetch(request); }
  targetDisposals() { return targetDisposals; }
}

class Retention extends RpcTarget {
  begin() {
    retentionBegins += 1;
    return { handle: crypto.randomUUID(), frame: crypto.randomUUID(), deadlineMs: 30_000 };
  }
  complete() { retentionCompletes += 1; }
  release() { retentionReleases += 1; }
}

class Controller extends RpcTarget {
  beginCapability() { return new Retention().begin(); }
  completeOperation() { retentionCompletes += 1; }
  releaseRetention() { retentionReleases += 1; }
  retainCapability() { return new Retention(); }
}

class SourceCapability extends RpcTarget {
  constructor(target, controller) {
    super();
    this.target = target;
    this.controller = typeof controller.dup === "function" ? controller.dup() : controller;
    this.retention = undefined;
  }

  activate(retention) {
    assert(this.retention === undefined, "capability activated twice");
    this.retention = retention.dup();
  }

  async call(operation, method, rawArgs) {
    assert(this.retention, "capability used without retention");
    const admission = await this.retention.begin();
    try {
      const args = decode(rawArgs);
      const value = operation === "get"
        ? Reflect.get(this.target, method, this.target)
        : method === "__call"
          ? Reflect.apply(this.target, undefined, args)
          : Reflect.apply(Reflect.get(this.target, method, this.target), this.target, args);
      const encoded = encode(await value, this.controller);
      await activateNested(encoded, this.controller, admission.handle);
      return encoded;
    } finally {
      await this.retention.complete(admission.handle);
    }
  }

  [Symbol.dispose]() {
    const retention = this.retention;
    this.retention = undefined;
    if (retention) Promise.resolve(retention.release()).finally(() => retention[Symbol.dispose]());
    if (typeof this.controller[Symbol.dispose] === "function") this.controller[Symbol.dispose]();
    if (typeof this.target[Symbol.dispose] === "function") this.target[Symbol.dispose]();
  }
}

async function activateNested(value, controller, operationHandle, seen = new WeakSet()) {
  if (!isObject(value) || seen.has(value)) return;
  seen.add(value);
  if (value.capability === 1) {
    await value.handle.activate(await controller.retainCapability(operationHandle));
    return;
  }
  if (!isPlain(value)) return;
  for (const item of Object.values(value)) {
    await activateNested(item, controller, operationHandle, seen);
  }
}

function encode(value, controller, seen = new WeakMap()) {
  if (value instanceof RpcTarget || typeof value === "function") {
    return {
      capability: 1,
      kind: typeof value === "function" ? "function" : "target",
      handle: new SourceCapability(value, controller),
    };
  }
  if (!isObject(value) || !isPlain(value)) return value;
  if (seen.has(value)) return seen.get(value);
  const output = Array.isArray(value) ? [] : Object.create(Object.getPrototypeOf(value));
  seen.set(value, output);
  for (const [key, item] of Object.entries(value)) output[key] = encode(item, controller, seen);
  return output;
}

function member(handle, property) {
  const call = (...args) => result(handle.call("call", property, args));
  return new Proxy(call, {
    get(target, key, receiver) {
      if (key === "then") {
        return (resolve, reject) => Promise.resolve(handle.call("get", property, [])).then(
          value => resolve(decode(value)), reject,
        );
      }
      return Reflect.get(target, key, receiver);
    },
  });
}

function capability(envelope) {
  const handle = envelope.handle;
  if (envelope.kind === "function") {
    const fn = (...args) => handle.call("call", "__call", args);
    return new Proxy(fn, {
      get(target, property, receiver) {
        if (property === "then") return undefined;
        if (property === "dup") return () => capability({ ...envelope, handle: handle.dup() });
        if (property === Symbol.dispose) return () => handle[Symbol.dispose]();
        return Reflect.get(target, property, receiver);
      },
    });
  }
  return new Proxy(Object.create(null), {
    get(_target, property) {
      if (property === "then") return undefined;
      if (property === "dup") return () => capability({ ...envelope, handle: handle.dup() });
      if (property === Symbol.dispose) return () => handle[Symbol.dispose]();
      return member(handle, property);
    },
  });
}

function decode(value, seen = new WeakMap()) {
  if (isObject(value) && value.capability === 1) return capability(value);
  if (!isObject(value) || !isPlain(value)) return value;
  if (seen.has(value)) return seen.get(value);
  const output = Array.isArray(value) ? [] : Object.create(Object.getPrototypeOf(value));
  seen.set(value, output);
  for (const [key, item] of Object.entries(value)) output[key] = decode(item, seen);
  return output;
}

async function activate(value, seen = new WeakSet()) {
  if (!isObject(value) || seen.has(value)) return;
  seen.add(value);
  if (value.capability === 1) {
    await value.handle.activate(new Retention());
    return;
  }
  if (!isPlain(value)) return;
  for (const item of Object.values(value)) await activate(item, seen);
}

export class Transport extends WorkerEntrypoint {
  async call(method, rawArgs) {
    await activate(rawArgs);
    const result = await this.env.BACKEND.invoke(method, rawArgs, new Controller());
    await activate(result);
    return result;
  }

  async get(property) {
    const result = await this.env.BACKEND.getProperty(property, new Controller());
    await activate(result);
    return result;
  }

  async fetch(request) { return this.env.BACKEND.fetch(request); }
  beginCapability() { return new Retention().begin(); }
  completeOperation() { retentionCompletes += 1; }
  releaseRetention() { retentionReleases += 1; }
  counts() { return { retentionBegins, retentionCompletes, retentionReleases }; }
}

function result(raw) {
  return new Proxy(raw, {
    get(target, property, receiver) {
      if (property === "then") {
        return (resolve, reject) => Reflect.apply(Reflect.get(target, "then"), target, [
          value => resolve(decode(value)), reject,
        ]);
      }
      const handle = Reflect.get(target, "handle");
      return member(handle, property);
    },
  });
}

function service(transport) {
  return new Proxy(Object.create(null), {
    get(_target, property) {
      if (property === "then") return undefined;
      if (property === "fetch") return request => transport.fetch(request);
      const item = (...args) => result(transport.call(property, encode(args, transport)));
      return new Proxy(item, {
        get(target, key, receiver) {
          if (key === "then") return (resolve, reject) => Promise.resolve(transport.get(property)).then(
            value => resolve(decode(value)), reject,
          );
          return Reflect.get(target, key, receiver);
        },
      });
    },
  });
}

class CallbackTarget extends RpcTarget {
  ping(value) { return `target-callback:${value}`; }
}

export default {
  async test(_controller, env) {
    const binding = service(env.TRANSPORT);
    assert(await binding.version === "backend-v1", "public getter failed");
    assert(await binding.scalar(41) === 42, "scalar failed");
    const structured = await binding.structured({ nested: [true, null, 7] });
    assert(structured.value.nested[2] === 7, "structured value changed");
    assert(structured.bytes instanceof Uint8Array && structured.bytes[2] === 3, "binary changed");
    assert(structured.when instanceof Date && structured.when.getTime() === 1_700_000_000_000, "Date changed");
    const request = await binding.request(new Request("https://original.example/path", { method: "POST" }));
    assert(request.url === "https://original.example/path" && request.method === "POST", "Request changed");
    const response = await binding.response();
    assert(response.status === 201 && await response.text() === "native-response", "Response changed");
    assert(await new Response(await binding.readable()).text() === "stream", "ReadableStream failed");
    const writer = (await binding.writable()).getWriter();
    await writer.write(new TextEncoder().encode("native-write"));
    await writer.close();
    assert(await binding.written() === "native-write", "WritableStream failed");
    assert(await binding.callback(value => `function:${value}`, "ok") === "function:ok", "function callback failed");
    assert(await binding.targetCallback(new CallbackTarget(), "ok") === "target-callback:ok", "RpcTarget callback failed");
    const upgrade = await binding.fetch(new Request("https://application.example/websocket", {
      headers: { Upgrade: "websocket" },
    }));
    assert(upgrade.status === 101 && upgrade.webSocket, "WebSocket upgrade failed");
    const socket = upgrade.webSocket;
    socket.accept();
    const echoed = new Promise((resolve, reject) => {
      socket.addEventListener("message", event => resolve(event.data), { once: true });
      socket.addEventListener("error", reject, { once: true });
    });
    socket.send("native-websocket");
    assert(await echoed === "echo:native-websocket", "WebSocket transport failed");
    socket.close(1000, "done");

    assert(await binding.capability("pipe").ping("ok") === "pipe:ok", "promise pipeline failed");
    assert(await binding.capability("pipe-get").label === "label:pipe-get", "pipelined getter failed");
    const target = await binding.capability("target");
    const duplicate = target.dup();
    assert(await target.ping("one") === "target:one", "returned RpcTarget failed");
    target[Symbol.dispose]();
    assert(await duplicate.ping("two") === "target:two", "dup released the shared target");
    const nested = await duplicate.nested();
    const nestedLabel = await nested.label;
    assert(nestedLabel === "label:target:nested", `nested capability/getter failed: ${nestedLabel}`);
    nested[Symbol.dispose]();
    duplicate[Symbol.dispose]();
    for (let attempt = 0; attempt < 20 && await env.BACKEND.targetDisposals() < 2; attempt += 1) {
      await scheduler.wait(5);
    }
    const counts = await env.TRANSPORT.counts();
    const disposalCount = await env.BACKEND.targetDisposals();
    assert(disposalCount >= 1, `target disposer did not run: ${disposalCount}; ${JSON.stringify(counts)}`);
    assert(counts.retentionBegins >= 4, "capability operations bypassed admission");
    assert(counts.retentionCompletes >= counts.retentionBegins, "capability completion was lost");
    assert(counts.retentionReleases >= 3, "capability retention did not release");

    let rejected = false;
    try { await binding.invalid(); } catch { rejected = true; }
    assert(rejected, "unsupported WeakMap was accepted");
  },
};
