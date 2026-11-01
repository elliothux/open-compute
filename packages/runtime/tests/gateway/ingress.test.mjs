import assert from "node:assert/strict";
import test from "node:test";
import { Ingress } from "./runtime.mjs";

function request(path, method = "GET", headers = {}, body) {
  return new Request(`http://private${path}`, {
    method,
    headers,
    body,
  });
}

test("private HTTP admission accepts credential-free probes and rejects malformed probes", async () => {
  const gateway = new Ingress({}, {});
  for (const path of ["/internal/ready", "/internal/live"]) {
    assert.equal((await gateway.fetch(request(path))).status, 204);
    for (const item of [
      request(path, "GET", { "content-type": "text/plain" }),
      request(path, "GET", { "content-length": "1" }),
      request(path + "?extra=1"),
      request(path, "POST", {}, "body"),
    ])
      assert.equal((await gateway.fetch(item)).status, 404);
  }
  assert.equal((await gateway.fetch(request("/not-internal"))).status, 404);
  const bodyProbe = request("/internal/live");
  bodyProbe.arrayBuffer = async () => new Uint8Array([1]).buffer;
  assert.equal((await gateway.fetch(bodyProbe)).status, 404);
});

test("delegated DO HTTP keeps identity and streams without credentials", async () => {
  const forwarded = [];
  const gateway = new Ingress(
    {},
    {
      DO_ROUTER: {
        fetch: async (req) => {
          forwarded.push(req);
          return new Response("do");
        },
      },
      LOADER_HOST: {
        fetch: async (req) => {
          forwarded.push(req);
          return new Response("loader");
        },
      },
    },
  );
  const NodeRequest = globalThis.Request;
  try {
    globalThis.Request = class extends NodeRequest {
      constructor(input, init) {
        super(input, { ...init, duplex: "half" });
      }
    };
    for (const method of ["GET", "POST"]) {
      const req = request(
        "/internal/do/v1/fetch",
        method,
        {
          "x-open-compute-do-order-channel": "channel",
          upgrade: "websocket",
        },
        method === "POST" ? "stream" : undefined,
      );
      assert.equal(await (await gateway.fetch(req)).text(), "do");
      const forwardedRequest = forwarded.at(-1);
      assert.equal(
        forwardedRequest.headers.has("x-open-compute-internal-token"),
        false,
      );
      assert.equal(
        forwardedRequest.headers.get("x-open-compute-do-order-channel"),
        "channel",
      );
      assert.equal(forwardedRequest.headers.get("upgrade"), "websocket");
      assert.equal(forwardedRequest.method, method);
      assert.equal(
        await forwardedRequest.text(),
        method === "POST" ? "stream" : "",
      );
    }
    for (const path of [
      "/internal/do-delete",
      "/internal/do-alarm",
      "/internal/do-alarm-repair",
      "/internal/container-start",
    ]) {
      assert.equal(
        await (await gateway.fetch(request(path, "POST", {}, "admin"))).text(),
        "do",
      );
      const req = forwarded.at(-1);
      assert.equal(req.url, `http://do-router${path}`);
      assert.deepEqual(
        [...req.headers],
        [["content-type", "application/json"]],
      );
      assert.equal(await req.text(), "admin");
    }
    assert.equal(
      await (
        await gateway.fetch(
          request("/internal/prepare-python", "POST", {}, "prepare"),
        )
      ).text(),
      "loader",
    );
    assert.equal(
      forwarded.at(-1).headers.get("x-open-compute-internal-token"),
      null,
    );
    assert.equal(await forwarded.at(-1).text(), "prepare");
    for (const req of [
      request("/internal/do/v1/fetch?extra=1"),
      request("/internal/do-delete"),
      request("/internal/container-start?extra=1", "POST", {}, "{}"),
    ]) {
      const count = forwarded.length;
      assert.equal((await gateway.fetch(req)).status, 404);
      assert.equal(forwarded.length, count);
    }
  } finally {
    globalThis.Request = NodeRequest;
  }
});

test("native DO RPC preserves response objects, capabilities and argument identity", async () => {
  const calls = [];
  const result = { capability: {} };
  const router = Object.fromEntries(
    [
      "dispatchFetch",
      "dispatchRpc",
      "getRpcProperty",
      "prepareConnect",
      "cancelOrder",
    ].map((name) => [
      name,
      (...args) => {
        calls.push([name, args]);
        return result;
      },
    ]),
  );
  const gateway = new Ingress({}, { DO_ROUTER: router });
  const identity = { version: "validated" };
  const req = new Request("http://tenant/path");
  const args = [{ key: "value" }];
  const authority = { kind: "record", hostname: "example.invalid", port: 443 };
  assert.equal(gateway.dispatchFetch(identity, req), result);
  assert.equal(gateway.dispatchRpc(identity, "method", args), result);
  assert.equal(gateway.getRpcProperty(identity, "property"), result);
  assert.equal(gateway.prepareConnect(identity, authority), result);
  assert.equal(gateway.cancelOrder(identity), result);
  for (const [, values] of calls) assert.equal(values[0], identity);
  assert.equal(calls[0][1][1], req);
  assert.equal(calls[1][1][2], args);
  assert.equal(calls[3][1][1], authority);
});

function socket(localAddress, bytes) {
  let closed = false;
  const received = [];
  return {
    opened: Promise.resolve({ localAddress }),
    closed: new Promise(() => {}),
    readable: new ReadableStream({
      start(controller) {
        controller.enqueue(new TextEncoder().encode(bytes));
        controller.close();
      },
    }),
    writable: new WritableStream({
      write(chunk) {
        received.push(new TextDecoder().decode(chunk));
      },
    }),
    close: async () => {
      closed = true;
    },
    received,
    isClosed: () => closed,
  };
}

test("delegated CONNECT tunnels bytes and sanitizes failures while closing its socket", async () => {
  const inbound = socket("nonce.do-router.invalid:1", "inbound");
  const outbound = socket("remote", "outbound");
  const gateway = new Ingress(
    {},
    {
      DO_ROUTER: {
        connect(address, options) {
          assert.equal(address, "nonce.do-router.invalid:1");
          assert.deepEqual(options, { allowHalfOpen: true });
          return outbound;
        },
      },
    },
  );
  await gateway.connect(inbound);
  assert.deepEqual(inbound.received, ["outbound"]);
  assert.deepEqual(outbound.received, ["inbound"]);
  for (const failure of ["address", "connect", "opened", "stream"]) {
    const input = socket(failure === "address" ? "\n" : "valid:1", "input");
    if (failure === "stream")
      input.readable = new ReadableStream({
        start(controller) {
          controller.error(new Error("private-token"));
        },
      });
    input.close = async () => {
      input.didClose = true;
      throw new Error("private-close-secret");
    };
    const target = socket("remote", "target");
    if (failure === "opened")
      target.opened = Promise.reject(new Error("private-source"));
    const worker = new Ingress(
      {},
      {
        DO_ROUTER: {
          connect() {
            if (failure === "connect") throw new Error("private-path");
            return target;
          },
        },
      },
    );
    await assert.rejects(
      worker.connect(input),
      (error) =>
        error.message === "DO_RUNTIME_EXCEPTION" &&
        error.stableCode === "DO_RUNTIME_EXCEPTION",
    );
    assert.equal(input.didClose, true);
  }
});
