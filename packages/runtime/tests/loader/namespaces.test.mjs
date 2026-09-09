import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { revokeWorkerLoaders } = await importRuntime("loader/namespaces.ts");
const { default: gateway } = await importRuntime("gateway/ingress.ts");
const path = "http://gateway/internal/worker-loaders/revoke";
const key = "a".repeat(64);
function request(value, overrides = {}) {
  const body = typeof value === "string" ? value : JSON.stringify(value);
  return new Request(path, {
    method: "POST",
    body,
    headers: {
      "content-type": "application/json",
      "content-length": String(Buffer.byteLength(body)),
      "x-open-compute-internal-token": "generation",
      ...overrides,
    },
  });
}

test("namespace revocation validates the whole bounded batch before mutating capabilities", async () => {
  const revoked = [];
  const factory = {
    revoke(value) {
      revoked.push(value);
    },
  };
  for (const value of [
    null,
    {},
    [],
    [key, 1],
    [key, "untrusted"],
    Array(129).fill(key),
    "{",
  ]) {
    assert.equal(
      (await revokeWorkerLoaders(request(value), factory)).status,
      400,
    );
    assert.deepEqual(revoked, []);
  }
  for (const headers of [
    { "content-type": "text/plain" },
    { "content-length": "0" },
    { "content-length": "16385" },
    { "content-length": "unknown" },
  ]) {
    assert.equal(
      (await revokeWorkerLoaders(request([key], headers), factory)).status,
      400,
    );
    assert.deepEqual(revoked, []);
  }
  assert.equal(
    (await revokeWorkerLoaders(request([key, "b".repeat(64)]), factory)).status,
    204,
  );
  assert.deepEqual(revoked, [key, "b".repeat(64)]);
});

test("only the authenticated private gateway can forward native namespace revocation", async (t) => {
  const NodeRequest = globalThis.Request;
  globalThis.Request = class extends NodeRequest {
    constructor(input, init) {
      super(input, { ...init, duplex: "half" });
    }
  };
  t.after(() => {
    globalThis.Request = NodeRequest;
  });
  const revoked = [];
  const env = {
    INTERNAL_TOKEN: "generation",
    LOADER_HOST: {
      fetch(req) {
        return revokeWorkerLoaders(req, {
          revoke(key) {
            revoked.push(key);
          },
        });
      },
    },
  };
  assert.equal(
    (
      await gateway.fetch(
        request([key], { "x-open-compute-internal-token": "old" }),
        env,
      )
    ).status,
    404,
  );
  assert.deepEqual(revoked, []);
  assert.equal(
    (await gateway.fetch(new Request(path + "?extra=1", request([key])), env))
      .status,
    404,
  );
  assert.deepEqual(revoked, []);
  assert.equal((await gateway.fetch(request([key]), env)).status, 204);
  assert.deepEqual(revoked, [key]);
});
