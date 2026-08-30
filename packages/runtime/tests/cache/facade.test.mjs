import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const calls = [];
const transport = {
  lookup: { status: "MISS", fenceGeneration: "1" },
  async match(namespace, name, request) {
    calls.push(["match", namespace, name, request.url, request.method, request.headers.get("range")]);
    return this.lookup;
  },
  async put(namespace, name, request, response, fence) {
    calls.push(["put", namespace, name, request.url, await response.text(), fence]);
  },
  async delete(namespace, name, request) {
    calls.push(["delete", namespace, name, request.url]);
    return true;
  },
  async purge(options) { calls.push(["purge", options]); return { success: true, deleted: 2 }; },
};
globalThis.__openComputeCacheEnv = { __OPEN_COMPUTE_PRIVATE_CACHE: { default: transport } };
const { createCacheRuntime } = await importRuntime("cache/facade.ts", {});
const automaticRuntime = failOpen => {
  const runtime = createCacheRuntime(true, failOpen).bind(globalThis.__openComputeCacheEnv);
  assert.ok(runtime);
  return runtime;
};
createCacheRuntime(false, true).bind(globalThis.__openComputeCacheEnv);

test("automatic cache construction defers request-scoped transport resolution", () => {
  const prior = globalThis.__openComputeCacheEnv.__OPEN_COMPUTE_PRIVATE_CACHE;
  delete globalThis.__openComputeCacheEnv.__OPEN_COMPUTE_PRIVATE_CACHE;
  try {
    assert.ok(createCacheRuntime(true, true));
  } finally {
    globalThis.__openComputeCacheEnv.__OPEN_COMPUTE_PRIVATE_CACHE = prior;
  }
});

test("default and named Cache API calls use strict isolated namespaces", async () => {
  calls.length = 0;
  transport.lookup = {
    status: "HIT", fenceGeneration: "2", response: new Response("cached", { headers: { etag: '"v1"' } }),
  };
  assert.equal(await (await caches.default.match("https://example.test/a")).text(), "cached");
  const named = await caches.open("rendered:pages");
  await named.put("https://example.test/b", new Response("stored", {
    headers: { "cache-control": "max-age=60" },
  }));
  assert.equal(await named.delete("https://example.test/b"), true);
  assert.deepEqual(calls.map(call => call.slice(0, 3)), [
    ["match", "default", undefined],
    ["put", "named", "rendered:pages"],
    ["delete", "named", "rendered:pages"],
  ]);
  await assert.rejects(named.put(
    new Request("https://example.test/b", { method: "POST" }),
    new Response("bad"),
  ), /CACHE_PUT_REJECTED/);
  await assert.rejects(caches.open("\n"), /CACHE_KEY_INVALID/);
});

test("automatic cache reports miss, hit, SWR refresh, SIE, bypass, and purge", async () => {
  calls.length = 0;
  const waits = [];
  const ctx = { waitUntil(promise) { waits.push(promise); } };
  const runtime = automaticRuntime(true);
  transport.lookup = { status: "MISS", fenceGeneration: "3" };
  let origins = 0;
  const miss = await runtime.dispatch(() => {
    origins += 1;
    return new Response("origin", { headers: { "cache-control": "max-age=60", "cache-tag": "gate" } });
  }, new Request("https://example.test/cache"), ctx);
  assert.equal(miss.headers.get("cf-cache-status"), "MISS");
  assert.equal(miss.headers.get("cache-tag"), null);
  assert.equal(await miss.text(), "origin");
  await Promise.all(waits.splice(0));
  assert.equal(origins, 1);
  assert.deepEqual(calls.at(-1).at(-1), { status: "MISS", fenceGeneration: "3" });

  transport.lookup = {
    status: "UPDATING", fenceGeneration: "3", refreshToken: "ab".repeat(16),
    response: new Response("stale", { headers: { "cf-cache-status": "UPDATING" } }),
  };
  const stale = await runtime.dispatch(() => {
    origins += 1;
    return new Response("fresh", { headers: { "cache-control": "max-age=60" } });
  }, new Request("https://example.test/cache"), ctx);
  assert.equal(await stale.text(), "stale");
  await Promise.all(waits.splice(0));
  assert.equal(origins, 2);

  transport.lookup = {
    status: "STALE_IF_ERROR", fenceGeneration: "4",
    response: new Response("fallback", { headers: { "cf-cache-status": "STALE_IF_ERROR" } }),
  };
  const fallback = await runtime.dispatch(() => new Response("error", { status: 503 }),
    new Request("https://example.test/cache"), ctx);
  assert.equal(fallback.headers.get("cf-cache-status"), "STALE");
  assert.equal(await fallback.text(), "fallback");

  const bypass = await runtime.dispatch(() => new Response("private", {
    headers: { "cache-control": "private, max-age=60" },
  }), new Request("https://example.test/private"), ctx);
  assert.equal(bypass.headers.get("cf-cache-status"), "BYPASS");
  assert.deepEqual(await runtime.context.purge({ tags: ["release"] }), { success: true, deleted: 2 });
});

test("automatic cache serves cached ranges and only bypasses an uncached partial origin response", async () => {
  calls.length = 0;
  const ctx = { waitUntil() { throw new Error("range response must not schedule a store"); } };
  const runtime = automaticRuntime(true);
  const request = new Request("https://example.test/range", {
    headers: { range: "bytes=1-2" },
  });
  transport.lookup = {
    status: "HIT",
    fenceGeneration: "7",
    response: new Response("bc", {
      status: 206,
      headers: { "content-range": "bytes 1-2/4", "cf-cache-status": "HIT" },
    }),
  };
  let origins = 0;
  const hit = await runtime.dispatch(() => {
    origins += 1;
    return new Response("origin");
  }, request, ctx);
  assert.equal(hit.status, 206);
  assert.equal(hit.headers.get("content-range"), "bytes 1-2/4");
  assert.equal(await hit.text(), "bc");
  assert.equal(origins, 0);
  assert.equal(calls.at(-1)[5], "bytes=1-2");

  transport.lookup = { status: "MISS", fenceGeneration: "8" };
  const miss = await runtime.dispatch(
    () => new Response("bc", { status: 206, headers: { "content-range": "bytes 1-2/4" } }),
    request,
    ctx,
  );
  assert.equal(miss.status, 206);
  assert.equal(miss.headers.get("cf-cache-status"), "BYPASS");
  assert.equal(calls.filter(call => call[0] === "put").length, 0);
});

test("automatic refresh and stale-if-error cancel hidden origin bodies", async () => {
  const waits = [];
  const ctx = { waitUntil(promise) { waits.push(promise); } };
  const runtime = automaticRuntime(true);
  let cancellations = 0;
  const hiddenBody = () => new ReadableStream({ cancel() { cancellations += 1; } });
  transport.lookup = {
    status: "UPDATING",
    fenceGeneration: "9",
    refreshToken: "cd".repeat(16),
    response: new Response("stale", { headers: { "cf-cache-status": "UPDATING" } }),
  };
  await runtime.dispatch(
    () => new Response(hiddenBody(), { headers: { "cache-control": "private" } }),
    new Request("https://example.test/refresh"),
    ctx,
  );
  await Promise.all(waits.splice(0));

  transport.lookup = {
    status: "STALE_IF_ERROR",
    fenceGeneration: "10",
    response: new Response("fallback", { headers: { "cf-cache-status": "STALE_IF_ERROR" } }),
  };
  const fallback = await runtime.dispatch(
    () => new Response(hiddenBody(), { status: 503 }),
    new Request("https://example.test/error"),
    ctx,
  );
  assert.equal(await fallback.text(), "fallback");
  assert.equal(cancellations, 2);
});

test("automatic cache availability follows the operator fail-open policy", async () => {
  const prior = transport.match;
  transport.match = async () => {
    throw Object.assign(new Error("CACHE_UNAVAILABLE"), { stableCode: "CACHE_UNAVAILABLE" });
  };
  try {
    const request = new Request("https://example.test/cache");
    const ctx = { waitUntil() {} };
    const bypass = await automaticRuntime(true).dispatch(
      () => new Response("origin"), request, ctx,
    );
    assert.equal(bypass.headers.get("cf-cache-status"), "BYPASS");
    await assert.rejects(
      automaticRuntime(false).dispatch(() => new Response("origin"), request, ctx),
      /CACHE_UNAVAILABLE/,
    );
  } finally {
    transport.match = prior;
  }
});

test("automatic cache parses field-qualified prohibitions and quoted TTLs", async () => {
  calls.length = 0;
  const waits = [];
  const ctx = { waitUntil(promise) { waits.push(promise); } };
  const runtime = automaticRuntime(true);
  for (const control of [
    'private="set-cookie", max-age=60',
    'no-cache="etag", max-age=60',
    "no-store=unexpected, max-age=60",
  ]) {
    transport.lookup = { status: "MISS", fenceGeneration: "5" };
    const response = await runtime.dispatch(
      () => new Response("private", { headers: { "cache-control": control } }),
      new Request("https://example.test/private"),
      ctx,
    );
    assert.equal(response.headers.get("cf-cache-status"), "BYPASS");
  }
  assert.equal(waits.length, 0);

  transport.lookup = { status: "MISS", fenceGeneration: "6" };
  const quoted = await runtime.dispatch(
    () => new Response("quoted", { headers: { "cache-control": 'max-age="60"' } }),
    new Request("https://example.test/quoted"),
    ctx,
  );
  assert.equal(quoted.headers.get("cf-cache-status"), "MISS");
  await Promise.all(waits);
  assert.equal(calls.at(-1)[0], "put");
});

test("automatic cache never fail-opens protocol or integrity failures", async () => {
  const prior = transport.match;
  const request = new Request("https://example.test/cache");
  const ctx = { waitUntil() {} };
  try {
    for (const code of ["CACHE_PROTOCOL_ERROR", "CACHE_CORRUPT"]) {
      transport.match = async () => { throw Object.assign(new Error(code), { stableCode: code }); };
      await assert.rejects(
        automaticRuntime(true).dispatch(() => new Response("origin"), request, ctx),
        new RegExp(code),
      );
    }
    const hostile = {};
    Object.defineProperty(hostile, "stableCode", { get() { throw new Error("secret"); } });
    transport.match = async () => { throw hostile; };
    await assert.rejects(
      automaticRuntime(true).dispatch(() => new Response("origin"), request, ctx),
      /CACHE_PROTOCOL_ERROR/,
    );
  } finally {
    transport.match = prior;
  }
});

test("cache lookup protocol requires canonical fences and status-response alignment", async () => {
  const prior = transport.lookup;
  const priorDelete = transport.delete;
  try {
    for (const lookup of [
      { status: "HIT", fenceGeneration: "8" },
      { status: "MISS", fenceGeneration: "8", response: new Response("unexpected") },
      { status: "MISS", fenceGeneration: "0" },
      { status: "UNKNOWN", fenceGeneration: "8" },
      { status: "UPDATING", fenceGeneration: "8", response: new Response("stale") },
      { status: "HIT", fenceGeneration: "8", refreshToken: "ab".repeat(16), response: new Response("hit") },
    ]) {
      transport.lookup = lookup;
      await assert.rejects(
        caches.default.match("https://example.test/malformed"),
        /CACHE_PROTOCOL_ERROR/,
      );
    }
    await assert.rejects(
      caches.default.put("https://example.test/malformed", {}),
      /CACHE_PROTOCOL_ERROR/,
    );
    transport.delete = async () => "false";
    await assert.rejects(
      caches.default.delete("https://example.test/malformed"),
      /CACHE_PROTOCOL_ERROR/,
    );
  } finally {
    transport.lookup = prior;
    transport.delete = priorDelete;
  }
});
