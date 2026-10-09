import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const shared = moduleUrl(`
  export const bindingError = code => new Error(code);
  export const resolveSnapshot = async () => ({ routeGeneration: 1 });
  export const doPolicy = () => ({});
  export const snapshotWorkerCode = () => ({});
  export const tenantGlobalOutbound = () => null;
`);
const protocol = moduleUrl(
  await compileRuntime("durable-objects/host-protocol.ts", {
    "./identity.js": moduleUrl(
      await compileRuntime("durable-objects/identity.ts"),
    ),
    "./errors.js": moduleUrl(await compileRuntime("durable-objects/errors.ts")),
    "../loader/shared.js": shared,
  }),
);
const { DoHost } = await importRuntime("durable-objects/host.ts", {
  "cloudflare:workers": moduleUrl(`
    export class DurableObject {
      constructor(ctx, env) { this.ctx = ctx; this.env = env; }
    }
  `),
  "../loader/bindings.js": moduleUrl("export const tenantEnv = () => ({});"),
  "../loader/modules.js": moduleUrl("export const modulesFor = () => ({});"),
  "../loader/shared.js": shared,
  "../observability/collector.js": moduleUrl(
    "export const collectableWorkerCode = code => code;",
  ),
  "../sockets/tunnel.js": moduleUrl(`
    export const inboundSocketAddress = async socket => socket.address;
    export const socketAddressFromWire = wire => wire.address;
    export const tunnelSockets = async () => {};
    export const validateSocketAuthorityWire = value => value;
  `),
  "./errors.js": moduleUrl(
    "export const sanitizeDoError = (_error, code) => new Error(code);",
  ),
  "./host-protocol.js": protocol,
});

const authority = Object.freeze({
  instanceId: "019c0000000070008000000000000001",
  workerId: "019c0000-0000-7000-8000-000000000002",
  versionId: "019c0000-0000-7000-8000-000000000003",
  workerCodeSha256: "a".repeat(64),
  namespaceResourceId: "019c0000-0000-7000-8000-000000000004",
  objectId: "b".repeat(64),
  objectGeneration: 1,
  routeGeneration: 1,
  className: "Root",
});
const descriptor = { native: true, id: "facet-id" };
const socketAuthority = { kind: "string", address: "example.com:443" };
const tokenFor = (index) => index.toString(16).padStart(32, "0");

function fixture() {
  const forwarded = [];
  const ctx = {
    id: "host-id",
    storage: { sql: { exec: () => ({ toArray: () => [] }) } },
    facets: {
      get: () => ({
        connect(address) {
          forwarded.push(address);
          return { opened: Promise.resolve() };
        },
      }),
    },
  };
  const env = {
    LOADER: { get: () => ({ getDurableObjectClass: () => ({}) }) },
    WORKER_LOADER_FACTORY: { getFacets: () => ({}) },
  };
  return { host: new DoHost(ctx, env), forwarded };
}

test("facet CONNECT cancellation frees capacity and is idempotent", async () => {
  const { host, forwarded } = fixture();
  const prepare = (index) =>
    host.__openComputePrepareFacetConnect(
      authority,
      ["child"],
      descriptor,
      tokenFor(index),
      socketAuthority,
    );
  for (let index = 0; index < 128; index++) await prepare(index);
  await assert.rejects(prepare(128), /DO_STORAGE_LIMIT/);
  await assert.rejects(
    host.__openComputeCancelFacetConnect("invalid-token"),
    /DO_INTERNAL_PROTOCOL_ERROR/,
  );
  await assert.rejects(prepare(128), /DO_STORAGE_LIMIT/);
  await host.__openComputeCancelFacetConnect(tokenFor(0));
  await host.__openComputeCancelFacetConnect(tokenFor(0));
  await prepare(128);
  const priorScheduler = globalThis.scheduler;
  globalThis.scheduler = { wait: async () => {} };
  try {
    await assert.rejects(
      host.connect({
        address: `${tokenFor(0)}.facet-connect.invalid:1`,
        close: async () => {},
      }),
      /DO_RUNTIME_EXCEPTION/,
    );
  } finally {
    globalThis.scheduler = priorScheduler;
  }
  assert.deepEqual(forwarded, []);
  await host.connect({
    address: `${tokenFor(128)}.facet-connect.invalid:1`,
    close: async () => {},
  });
  assert.deepEqual(forwarded, ["example.com:443"]);
  await host.__openComputeCancelFacetConnect(tokenFor(128));
});

test("facet cancellation does not revoke a tenant CONNECT handoff", async () => {
  const { host } = fixture();
  const tenantToken = await host.__openComputePrepareConnect(
    authority,
    { channelId: "c".repeat(32), sequence: 0 },
    socketAuthority,
  );
  for (let index = 0; index < 127; index++)
    await host.__openComputePrepareFacetConnect(
      authority,
      ["child"],
      descriptor,
      tokenFor(index),
      socketAuthority,
    );
  await host.__openComputeCancelFacetConnect(tenantToken);
  await assert.rejects(
    host.__openComputePrepareFacetConnect(
      authority,
      ["child"],
      descriptor,
      tokenFor(128),
      socketAuthority,
    ),
    /DO_STORAGE_LIMIT/,
  );
});

for (const outcome of ["prepared", "cancelled", "timed out"]) {
  test(`facet CONNECT waits for cold startup until ${outcome}`, async () => {
    const { host, forwarded } = fixture();
    const entered = Promise.withResolvers();
    const expired = Promise.withResolvers();
    let timerCancelled = false;
    const priorScheduler = globalThis.scheduler;
    globalThis.scheduler = {
      wait(_duration, { signal }) {
        entered.resolve();
        return new Promise((resolve, reject) => {
          expired.promise.then(resolve);
          signal.addEventListener("abort", () => {
            timerCancelled = true;
            reject(signal.reason);
          });
        });
      },
    };
    try {
      const token = tokenFor(99);
      let closed = false;
      const connected = host.connect({
        address: `${token}.facet-connect.invalid:1`,
        close: async () => {
          closed = true;
        },
      });
      const observed = connected.then(
        () => "completed",
        () => "rejected",
      );
      assert.equal(
        await Promise.race([observed, entered.promise.then(() => "waiting")]),
        "waiting",
      );
      if (outcome === "prepared") {
        await host.__openComputePrepareFacetConnect(
          authority,
          ["child"],
          descriptor,
          token,
          socketAuthority,
        );
        await connected;
        assert.deepEqual(forwarded, ["example.com:443"]);
        assert.equal(closed, false);
      } else {
        if (outcome === "cancelled")
          await host.__openComputeCancelFacetConnect(token);
        else expired.resolve();
        await assert.rejects(connected, /DO_RUNTIME_EXCEPTION/);
        assert.deepEqual(forwarded, []);
        assert.equal(closed, true);
      }
      assert.equal(timerCancelled, true);
    } finally {
      globalThis.scheduler = priorScheduler;
    }
  });
}

test("a duplicate facet CONNECT cannot replace the original waiter", async () => {
  const { host, forwarded } = fixture();
  const entered = Promise.withResolvers();
  const priorScheduler = globalThis.scheduler;
  globalThis.scheduler = {
    wait(_duration, { signal }) {
      entered.resolve();
      return new Promise((_resolve, reject) => {
        signal.addEventListener("abort", () => reject(signal.reason));
      });
    },
  };
  try {
    const token = tokenFor(100);
    const socket = () => ({
      address: `${token}.facet-connect.invalid:1`,
      close: async () => {},
    });
    const original = host.connect(socket());
    await entered.promise;
    await assert.rejects(host.connect(socket()), /DO_RUNTIME_EXCEPTION/);
    await host.__openComputePrepareFacetConnect(
      authority,
      ["child"],
      descriptor,
      token,
      socketAuthority,
    );
    await original;
    assert.deepEqual(forwarded, ["example.com:443"]);
  } finally {
    globalThis.scheduler = priorScheduler;
  }
});
