import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const metadata = new WeakMap();
globalThis.__openComputeFacetTestMetadata = metadata;
const background = [];
const cloudflare = moduleUrl(`
  export function waitUntil(promise) { globalThis.__openComputeFacetTestBackground.push(Promise.resolve(promise)); }
`);
globalThis.__openComputeFacetTestBackground = background;
const runtime = moduleUrl(`
  export function loopbackDurableObjectMetadata(value) {
    return globalThis.__openComputeFacetTestMetadata.get(value);
  }
`);
const tunnel = moduleUrl(`
  export function socketAuthorityWire(value) { return { kind: "test", value }; }
`);
const { TenantFacets } = await importRuntime("durable-objects/facets.ts", {
  "cloudflare:workers": cloudflare,
  "../loader/wrappers/runtime.js": runtime,
  "../sockets/tunnel.js": tunnel,
});

const authority = Object.freeze({
  accountId: "019c0000-0000-7000-8000-000000000001",
  workerId: "019c0000-0000-7000-8000-000000000002",
  deploymentId: "019c0000-0000-7000-8000-000000000003",
  workerCodeSha256: "a".repeat(64),
  objectId: "b".repeat(64),
  namespaceResourceId: "019c0000-0000-7000-8000-000000000004",
  objectGeneration: 1,
  routeGeneration: 1,
  className: "Root",
});

function fixture() {
  const calls = [];
  const manager = {
    async __openComputeFacetCall(_authority, path, descriptor, method, args) {
      calls.push({ kind: "call", path, descriptor, method, args });
      return method === "increment" ? 1 : { method, args };
    },
    async __openComputeFacetGet(_authority, path, descriptor, property) {
      calls.push({ kind: "get", path, descriptor, property });
      return property === "label" ? "facet-label" : undefined;
    },
    async __openComputeFacetFetch(_authority, path, descriptor, request) {
      calls.push({ kind: "fetch", path, descriptor, url: request.url });
      return new Response("facet-response");
    },
    async __openComputeFacetAbort(_authority, path, name, reason) {
      calls.push({ kind: "abort", path, name, reason });
    },
    async __openComputeFacetDelete(_authority, path, name) {
      calls.push({ kind: "delete", path, name });
    },
    async __openComputeFacetClone(_authority, path, source, destination) {
      calls.push({ kind: "clone", path, source, destination });
    },
    async __openComputePrepareFacetConnect(_authority, path, descriptor, token, socket) {
      calls.push({ kind: "prepare-connect", path, descriptor, token, socket });
    },
    connect(address, options) {
      calls.push({ kind: "connect", address, options });
      return { address, options, opened: Promise.resolve(), closed: Promise.resolve() };
    },
  };
  return { calls, facets: new TenantFacets(manager, authority, [], "root-id"), manager };
}

function loopback(entrypoint, props) {
  const value = Object.freeze({});
  metadata.set(value, { entrypoint, props });
  return value;
}

test("logical facets forward methods, properties, fetch, props, and inherited ids", async () => {
  const { calls, facets } = fixture();
  let startups = 0;
  const stub = facets.get("child", () => {
    startups += 1;
    return { class: loopback("Child", { marker: "value" }) };
  });
  assert.equal(await stub.increment(), 1);
  assert.equal(await stub.label, "facet-label");
  assert.equal(await (await stub.fetch("https://facet.invalid/path")).text(), "facet-response");
  assert.equal(startups, 1);
  assert.deepEqual(calls.map(call => [call.kind, call.path]), [
    ["call", ["child"]], ["get", ["child"]], ["fetch", ["child"]],
  ]);
  assert.deepEqual(calls[0].descriptor, {
    entrypoint: "Child", id: "root-id", props: { marker: "value" },
  });
});

test("clone and delete are ordered before destination startup and clear cached callbacks", async () => {
  const { calls, facets, manager } = fixture();
  let releaseClone;
  manager.__openComputeFacetClone = async (...args) => {
    calls.push({ kind: "clone", args });
    await new Promise(resolve => { releaseClone = resolve; });
  };
  let startups = 0;
  const sourceClass = loopback("Child", undefined);
  await facets.get("source", () => ({ class: sourceClass })).increment();
  facets.clone("source", "destination");
  const pending = facets.get("destination", () => {
    startups += 1;
    return { class: sourceClass, id: "destination-id" };
  }).increment();
  while (releaseClone === undefined) await Promise.resolve();
  assert.equal(startups, 0);
  releaseClone();
  assert.equal(await pending, 1);
  assert.equal(startups, 1);
  facets.delete("destination");
  const fresh = facets.get("destination", () => {
    startups += 1;
    return { class: sourceClass, id: "fresh-id" };
  }).increment();
  assert.equal(await fresh, 1);
  assert.equal(startups, 2);
  await Promise.all(background.splice(0));
  assert.deepEqual(calls.filter(call => call.kind === "clone").length, 1);
  assert.deepEqual(calls.filter(call => call.kind === "delete").length, 1);
});

test("abort rejects existing stubs with the caller reason and permits a fresh startup", async () => {
  const { calls, facets } = fixture();
  const childClass = loopback("Child", undefined);
  const stale = facets.get("child", () => ({ class: childClass, id: "first" }));
  assert.equal(await stale.increment(), 1);
  const reason = new Error("facet-aborted");
  facets.abort("child", reason);
  await assert.rejects(stale.increment(), error => error === reason);
  const fresh = facets.get("child", () => ({ class: childClass, id: "second" }));
  assert.equal(await fresh.increment(), 1);
  await Promise.all(background.splice(0));
  assert.equal(calls.filter(call => call.kind === "abort").length, 1);
  assert.equal(calls.filter(call => call.kind === "call").length, 2);
});

test("facet validation and exposed surface match the Cloudflare contract", async () => {
  const { calls, facets } = fixture();
  assert.deepEqual(Reflect.ownKeys(facets), []);
  assert.deepEqual(
    Object.getOwnPropertyNames(Object.getPrototypeOf(facets)).sort(),
    ["abort", "clone", "constructor", "delete", "get"],
  );
  assert.throws(() => facets.get("x".repeat(257), () => ({ class: loopback("Child") })), /too long/);
  assert.throws(
    () => new TenantFacets({}, authority, ["a", "b", "c"], "id")
      .get("d", () => ({ class: loopback("Child") })),
    /depth limit/,
  );
  await assert.rejects(
    facets.get("invalid", () => ({ class: {} })).increment(),
    /DO_RUNTIME_EXCEPTION/,
  );
  const connected = facets.get("socket", () => ({ class: loopback("Child") }))
    .connect("example.com:443", { allowHalfOpen: true });
  assert.match(connected.address, /^[0-9a-f]{32}\.facet-connect\.invalid:1$/);
  await Promise.all(background.splice(0));
  assert.equal(calls.some(call => call.kind === "prepare-connect"), true);
});
