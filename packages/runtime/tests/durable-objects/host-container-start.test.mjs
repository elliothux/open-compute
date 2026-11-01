import assert from "node:assert/strict";
import test from "node:test";
import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const shared = moduleUrl(`
  export const bindingError = code => new Error(code);
  export const resolveSnapshot = async () => {
    const state = globalThis.__containerStartFixture;
    await state.snapshotBarrier?.();
    return state.snapshot;
  };
  export const snapshotWorkerCode = async () => {
    const state = globalThis.__containerStartFixture;
    state.codeReads++;
    if (state.failCode) throw new Error('controlled-code-failure');
    return {};
  };
  export const doPolicy = () => ({});
  export const tenantGlobalOutbound = () => null;
  export const currentStartupGeneration = () => 'generation';

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
  "../containers/attachment.js": moduleUrl(
    await compileRuntime("containers/attachment.ts", {
      "../loader/shared.js": shared,
    }),
  ),
  "cloudflare:workers": moduleUrl(
    "export class DurableObject { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }",
  ),
  "../loader/bindings.js": moduleUrl(
    "export const tenantEnv = () => ({openComputePrivateEnv: {}});",
  ),
  "../loader/modules.js": moduleUrl(
    "export const modulesFor = () => ({mainModule: 'tenant.js', modules: []});",
  ),
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
const declaration = Object.freeze({
  className: "Root",
  namespaceResourceId: authority.namespaceResourceId,
  applicationName: "containers",
  descriptorSha256: "c".repeat(64),
  images: {},
});
function startRequest(value = authority, operation = "container-start") {
  const headers = { "x-open-compute-do-operation": operation };
  for (const [key, fieldValue] of Object.entries(value)) {
    const name = key.replace(/[A-Z]/g, (letter) => `-${letter.toLowerCase()}`);
    headers[`x-open-compute-${name}`] = String(fieldValue);
  }
  return new Request("http://host/internal/container-start", { headers });
}
function fixture(containerClasses = [declaration]) {
  const state = {
    snapshot: { routeGeneration: 1, containerClasses },
    codeReads: 0,
    grants: [],
    loaders: [],
    facets: new Map(),
    revocations: [],
    failLoader: false,
    failClass: false,
    failCode: false,
  };
  globalThis.__containerStartFixture = state;
  let meta;
  const registered = new Map();
  const ctx = {
    id: "fixture-host-id",
    storage: {
      sql: {
        exec(query, ...args) {
          if (query.startsWith("SELECT route_generation"))
            return { toArray: () => (meta ? [meta] : []) };
          if (query.startsWith("INSERT OR REPLACE INTO open_compute_host_meta"))
            meta = {
              route_generation: args[0],
              version_id: args[1],
              object_generation: args[2],
              data_format_version: args[3],
              object_name: args[4],
              jurisdiction: args[5],
            };
          if (query.startsWith("INSERT INTO open_compute_host_facets"))
            registered.set(args[0], args[1]);
          if (query.startsWith("SELECT logical_path_json"))
            return {
              one: () => ({ logical_path_json: registered.get(args[0]) }),
            };
          if (query.startsWith("SELECT physical_name"))
            return {
              toArray: () =>
                Array.from(
                  registered,
                  ([physical_name, logical_path_json]) => ({
                    physical_name,
                    logical_path_json,
                  }),
                ),
            };
          if (query.startsWith("DELETE FROM open_compute_host_facets"))
            registered.delete(args[0]);
          return { toArray: () => [] };
        },
      },
    },
    facets: {
      get(name, getInfo) {
        if (!state.facets.has(name)) {
          const facet = {
            info: getInfo(),
            label: "child",
            attached: false,
            async ensureAttached() {
              this.code ??= await this.info.class.getCode();
              this.attached = true;
              return this.code;
            },
            async __openComputeInitialize() {
              await state.initializeBarrier?.();
              await this.ensureAttached();
            },
          };
          state.facets.set(name, facet);
        }
        return state.facets.get(name);
      },
      async abort(name) {
        state.facets.delete(name);
      },
      async delete(name) {
        state.facets.delete(name);
      },
    },
    exports: { AlarmIndex: () => ({}), FacetManager: () => ({}) },
  };
  const env = {
    NATIVE_DO_ID: { create: (id) => id },
    LOADER: {
      get(name, getCode) {
        assert.equal(name, null);
        if (state.failLoader) throw new Error("controlled-loader-failure");
        // A class handle does not run getCode; native start later ensures the attached actor.
        const loaded = {
          getDurableObjectClass: (entrypoint) => {
            if (state.failClass) throw new Error("controlled-class-failure");
            return { entrypoint, getCode };
          },
        };
        state.loaders.push(loaded);
        return loaded;
      },
    },
    WORKER_LOADER_FACTORY: {
      getFacets: () => ({ revoke() {} }),
      getContainer(facets, className, resolve, capture, images) {
        assert.equal(facets, ctx.facets);
        assert.equal(className, "Root");
        assert.equal(typeof resolve, "function");
        assert.equal(typeof capture, "function");
        assert.deepEqual(images, {});
        const grant = {
          revoked: false,
          starts: 0,
          async start() {
            if (this.revoked) throw new Error("controlled-grant-revoked");
            const facet = state.facets.get("tenant");
            assert.equal(
              facet.attached,
              true,
              "native tenant facet must exist",
            );
            assert.equal(facet.code.openComputeContainer, this);
            this.starts++;
          },
          revoke() {
            assert.equal(this.revoked, false);
            this.revoked = true;
            state.revocations.push(this);
          },
        };
        state.grants.push(grant);
        return grant;
      },
    },
  };
  return { host: new DoHost(ctx, env), state };
}

test("cold control start issues one grant before the lazy code callback", async () => {
  const { host, state } = fixture();
  assert.deepEqual(await (await host.fetch(startRequest())).json(), {
    started: true,
  });
  assert.equal(state.grants.length, 1);
  assert.equal(state.codeReads, 1);
  assert.equal(state.grants[0].starts, 1);
  assert.deepEqual(await (await host.fetch(startRequest())).json(), {
    started: true,
  });
  assert.equal(state.grants.length, 1);
  assert.equal(state.codeReads, 1);
  assert.equal(state.grants[0].starts, 2);
});

test("route advance during initialization cannot start the replacement grant for stale authority", async () => {
  const { host, state } = fixture();
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let entered;
  const ready = new Promise((resolve) => {
    entered = resolve;
  });
  let calls = 0;
  state.initializeBarrier = () => {
    if (++calls !== 1) return;
    entered();
    return held;
  };
  const first = host.fetch(startRequest()).then(
    () => assert.fail("revoked grant started"),
    (error) => assert.match(error.message, /controlled-grant-revoked/),
  );
  await ready;
  const advanced = {
    ...authority,
    routeGeneration: 2,
    versionId: "019c0000-0000-7000-8000-000000000005",
  };
  state.snapshot = { ...state.snapshot, routeGeneration: 2 };
  await host.fetch(startRequest(advanced));
  release();
  await first;
  assert.equal(state.grants[0].revoked, true);
  assert.equal(state.grants[0].starts, 0);
  assert.equal(state.grants[1].revoked, false);
  assert.equal(state.grants[1].starts, 1);
});

test("undeclared classes do not issue grants or trigger lazy code", async () => {
  const { host, state } = fixture([]);
  await assert.rejects(host.fetch(startRequest()), /CONTAINER_UNAVAILABLE/);
  assert.equal(state.grants.length, 0);
  assert.equal(state.codeReads, 0);
});

test("a nested declared class cannot receive a root container grant", async () => {
  const { host, state } = fixture([
    declaration,
    { ...declaration, className: "Child" },
  ]);
  assert.equal(
    await host.__openComputeFacetGet(
      authority,
      ["child"],
      { entrypoint: "Child", id: "child", props: {} },
      "label",
    ),
    "child",
  );
  assert.equal(state.grants.length, 1);
  assert.equal(state.codeReads, 0);
  await Array.from(state.facets.values())[1].ensureAttached();
  assert.equal(state.grants.length, 1);
  assert.equal(
    Array.from(state.facets.values())[1].code.openComputeContainer,
    undefined,
  );
});

test("route advance and delete revoke the issued grant without duplicate issuance", async () => {
  const { host, state } = fixture();
  await host.fetch(startRequest());
  const first = state.grants[0];
  const advanced = {
    ...authority,
    routeGeneration: 2,
    versionId: "019c0000-0000-7000-8000-000000000005",
  };
  state.snapshot = { ...state.snapshot, routeGeneration: 2 };
  await host.fetch(startRequest(advanced));
  assert.equal(first.revoked, true);
  assert.equal(state.grants.length, 2);
  assert.equal(state.grants[1].starts, 1);
  await host.fetch(startRequest(advanced));
  assert.equal(state.grants.length, 2);
  await host.fetch(startRequest(advanced, "delete"));
  assert.deepEqual(state.revocations, state.grants);
});

for (const failure of ["failLoader", "failClass", "failCode"]) {
  test(`failed ${failure} acquisition revokes only its issued grant`, async () => {
    const { host, state } = fixture();
    state[failure] = true;
    await assert.rejects(host.fetch(startRequest()), /controlled-/);
    assert.equal(state.grants.length, 1);
    assert.equal(state.grants[0].revoked, true);
    assert.equal(state.grants[0].starts, 0);
    await host.fetch(startRequest(authority, "delete"));
    assert.equal(state.revocations.length, 1);
  });
}

test("concurrent advance shares one issued grant", async () => {
  const { host, state } = fixture();
  await host.fetch(startRequest());
  const advanced = {
    ...authority,
    routeGeneration: 2,
    versionId: "019c0000-0000-7000-8000-000000000005",
  };
  state.snapshot = { ...state.snapshot, routeGeneration: 2 };
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let entered = 0;
  let firstEntered;
  const firstReady = new Promise((resolve) => {
    firstEntered = resolve;
  });
  state.snapshotBarrier = () => {
    if (++entered === 1) firstEntered();
    return held;
  };
  const first = host.fetch(startRequest(advanced));
  await firstReady;
  const second = host.fetch(startRequest(advanced));
  await new Promise((resolve) => setImmediate(resolve));
  release();
  const results = await Promise.allSettled([first, second]);
  assert.equal(
    state.grants.filter((grant) => !grant.revoked).length,
    1,
    "one current grant per declared root class",
  );
  assert.deepEqual(
    results.map((result) => result.status),
    ["fulfilled", "fulfilled"],
  );
});

test("queued stale authority cannot revoke the advanced grant", async () => {
  const { host, state } = fixture();
  await host.fetch(startRequest());
  const advanced = {
    ...authority,
    routeGeneration: 2,
    versionId: "019c0000-0000-7000-8000-000000000005",
  };
  state.snapshot = { ...state.snapshot, routeGeneration: 2 };
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let entered;
  const ready = new Promise((resolve) => {
    entered = resolve;
  });
  state.snapshotBarrier = () => {
    entered();
    return held;
  };
  const advancing = host.fetch(startRequest(advanced));
  await ready;
  const stale = host.fetch(startRequest()).then(
    () => assert.fail("stale authority accepted"),
    (error) => assert.match(error.message, /DO_VERSION_STALE/),
  );
  release();
  await advancing;
  await stale;
  assert.equal(state.grants.filter((grant) => !grant.revoked).length, 1);
  assert.equal(state.grants.at(-1).starts, 1);
});

test("deletion waits for class acquisition and revokes its grant", async () => {
  const { host, state } = fixture();
  let release;
  const held = new Promise((resolve) => {
    release = resolve;
  });
  let entered;
  const ready = new Promise((resolve) => {
    entered = resolve;
  });
  state.snapshotBarrier = () => {
    entered();
    return held;
  };
  const loading = host.fetch(startRequest());
  await ready;
  const deleting = host.fetch(startRequest(authority, "delete"));
  release();
  const results = await Promise.allSettled([loading, deleting]);
  assert.equal(results[1].status, "fulfilled");
  assert.equal(state.grants.length, 1);
  assert.equal(state.grants[0].revoked, true);
  assert.equal(state.facets.has("tenant"), false);
});
