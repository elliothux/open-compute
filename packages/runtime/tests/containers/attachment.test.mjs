import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const shared = moduleUrl(`
  export function bindingError(code) { return new Error(code); }
  export function currentStartupGeneration() { return 'generation'; }

`);
const {
  containerAttachment,
  containerLeaseFromWire,
  containerSnapshotFromWire,
  containerStartupFromWire,
} = await importRuntime("containers/attachment.ts", {
  "../loader/shared.js": shared,
});

test("snapshot wire admits only bounded public metadata", () => {
  const snapshot = { id: "opaque-snapshot", size: 0, name: "checkpoint" };
  assert.deepEqual(containerSnapshotFromWire({ snapshot }), snapshot);
  assert.deepEqual(
    containerSnapshotFromWire({ snapshot: { id: "other", size: 12 } }),
    {
      id: "other",
      size: 12,
    },
  );
  for (const value of [
    undefined,
    {},
    { snapshot: null },
    { snapshot: [] },
    ...[
      { id: "" },
      { id: "x".repeat(257) },
      { id: "a\0b" },
      { size: -1 },
      { size: 0.1 },
      { size: Number.MAX_SAFE_INTEGER + 1 },
      { size: "12" },
      { name: null },
      { archivePath: "/private/archive" },
    ].map((patch) => ({ snapshot: { ...snapshot, ...patch } })),
  ])
    assert.throws(
      () => containerSnapshotFromWire(value),
      /VERSION_INVARIANT_VIOLATION/,
    );
});
const lease = {
  opaqueEngineKey: "a".repeat(64),
  imageRef: `sha256:${"b".repeat(64)}`,
  providerContractSha256: "c".repeat(64),
  controlToken: "e".repeat(64),
  callbackToken: "f".repeat(64),
  nativeStartupJson: JSON.stringify({ enableInternet: false }),
  inspection: {
    image: `image@sha256:${"d".repeat(64)}`,
    labels: [{ name: "purpose", value: "state" }],
  },
  leaseGeneration: 2,
  targetGeneration: 3,
  state: "running",
  stopRequestedAtMs: null,
};
const authority = {
  instanceId: "instance",
  namespaceResourceId: "namespace",
  className: "App",
  objectId: "object",
  objectGeneration: 4,
  routeGeneration: 5,
};
const declaration = {
  className: "App",
  namespaceResourceId: "namespace",
  applicationName: "app",
  descriptorSha256: "d".repeat(64),
  images: {},
};

test("Container named images reach only the trusted native grant", () => {
  const images = { api: `registry/app@sha256:${"1".repeat(64)}` };
  let argumentsPassed;
  containerAttachment(
    {
      WORKER_LOADER_FACTORY: {
        getContainer(...args) {
          argumentsPassed = args;
        },
      },
    },
    { facets: {} },
    authority,
    { ...declaration, images },
  );
  assert.equal(argumentsPassed[1], "App");
  assert.deepEqual(argumentsPassed[4], images);
});

test("snapshot capture binds the current running lease and frozen facet authority", async () => {
  const requests = [];
  let capture;
  let current = lease;
  const snapshot = { id: "opaque-snapshot", size: 128, name: "checkpoint" };
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_facets, _class, _resolve, callback) {
        capture = callback;
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url, init) {
        requests.push({
          url,
          body: JSON.parse(init.body),
          headers: init.headers,
        });
        return Response.json(
          url.endsWith("resolve") ? { lease: current } : { snapshot },
        );
      },
    },
  };
  const mutable = { ...authority };
  containerAttachment(env, { facets: {} }, mutable, declaration);
  mutable.routeGeneration = 99;
  assert.deepEqual(
    await capture(lease.opaqueEngineKey, "checkpoint"),
    snapshot,
  );
  assert.ok(requests.at(-1).url.endsWith("/snapshot"));
  assert.equal(requests.at(-1).body.source.routeGeneration, 5);
  assert.equal(requests.at(-1).body.source.leaseGeneration, 2);
  assert.equal(
    requests.at(-1).body.source.opaqueEngineKey,
    lease.opaqueEngineKey,
  );
  assert.equal(requests.at(-1).body.name, "checkpoint");
  assert.equal(
    requests.at(-1).headers["x-open-compute-internal-token"],
    undefined,
  );
  await capture(lease.opaqueEngineKey, undefined);
  assert.equal(Object.hasOwn(requests.at(-1).body, "name"), false);
  for (current of [
    null,
    { ...lease, state: "lost" },
    { ...lease, stopRequestedAtMs: 1 },
    { ...lease, opaqueEngineKey: "b".repeat(64) },
  ]) {
    const before = requests.filter(({ url }) =>
      url.endsWith("/snapshot"),
    ).length;
    await assert.rejects(
      capture(lease.opaqueEngineKey, undefined),
      /CONTAINER_UNAVAILABLE/,
    );
    assert.equal(
      requests.filter(({ url }) => url.endsWith("/snapshot")).length,
      before,
    );
  }
  const count = requests.length;
  await assert.rejects(
    capture("bad", undefined),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.equal(requests.length, count);
});

test("native lease wire excludes mutable policy and rejects malformed execution authority", () => {
  assert.equal(containerLeaseFromWire({ lease: null }), undefined);
  assert.deepEqual(containerLeaseFromWire({ lease }), {
    leaseKey: lease.opaqueEngineKey,
    imageRef: lease.imageRef,
    providerContractSha256: lease.providerContractSha256,
    controlToken: lease.controlToken,
    callbackToken: lease.callbackToken,
    nativeStartupJson: lease.nativeStartupJson,
    inspection: lease.inspection,
  });
  for (const invalid of [
    undefined,
    [],
    {},
    { lease: [] },
    ...[
      { opaqueEngineKey: "A".repeat(64) },
      { imageRef: "image:latest" },
      { providerContractSha256: "bad" },
      { controlToken: "bad" },
      { nativeStartupJson: undefined },
      { nativeStartupJson: "x".repeat(1048577) },
      { inspection: undefined },
      { inspection: { image: 1, labels: [] } },
      { inspection: { image: "a b", labels: [] } },
      { inspection: { image: "x".repeat(1025), labels: [] } },
      { inspection: { image: "", labels: [], engineId: "private" } },
      {
        inspection: {
          image: "",
          labels: Array(11).fill({ name: "a", value: "b" }),
        },
      },
      {
        inspection: {
          image: "",
          labels: [{ name: "a", value: "b", private: true }],
        },
      },
      {
        inspection: {
          image: "",
          labels: [{ name: "dev.open-compute.owner", value: "b" }],
        },
      },
      {
        inspection: {
          image: "",
          labels: [
            { name: "a", value: "b" },
            { name: "a", value: "c" },
          ],
        },
      },
      { inspection: { image: "", labels: [{ name: "", value: "b" }] } },
      { inspection: { image: "", labels: [{ name: "a", value: "b\0c" }] } },
      {
        inspection: {
          image: "",
          labels: [{ name: "a", value: "x".repeat(65) }],
        },
      },
      {
        inspection: {
          image: "",
          labels: [{ name: "名".repeat(6), value: "b" }],
        },
      },
      {
        inspection: {
          image: "",
          labels: [{ name: "a", value: "值".repeat(22) }],
        },
      },
      { callbackToken: "bad" },
      { callbackToken: lease.controlToken },
      { leaseGeneration: 0 },
      { leaseGeneration: "2" },
      { targetGeneration: Number.MAX_SAFE_INTEGER + 1 },
      { state: "stopped" },
      { stopRequestedAtMs: undefined },
    ].map((patch) => ({ lease: { ...lease, ...patch } })),
  ])
    assert.throws(
      () => containerLeaseFromWire(invalid),
      /VERSION_INVARIANT_VIOLATION/,
    );
});

test("class attachment freezes authority and persists stop intent before final cleanup", async () => {
  const requests = [];
  let resolve;
  const grant = { revoke() {} };
  const facets = {};
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(actualFacets, className, callback) {
        assert.equal(actualFacets, facets);
        assert.equal(className, "App");
        resolve = (
          start,
          stop,
          completed,
          startup = "",
          expectedLeaseKey = "",
          monitor = false,
        ) =>
          callback(start, stop, completed, startup, expectedLeaseKey, monitor);
        return grant;
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url, init) {
        requests.push({
          url,
          body: JSON.parse(init.body),
          headers: init.headers,
        });
        return url.endsWith("resolve")
          ? Response.json({ lease })
          : new Response(null, { status: 204 });
      },
    },
  };
  const mutable = { ...authority };
  assert.equal(
    containerAttachment(env, { facets }, mutable, declaration),
    grant,
  );
  mutable.routeGeneration = 99;
  assert.deepEqual(
    await resolve(true, false, false),
    containerLeaseFromWire({ lease }),
  );
  assert.equal(requests[0].body.start, true);
  assert.equal(Object.hasOwn(requests[0].body, "startup"), false);
  assert.equal(requests[0].body.routeGeneration, 5);
  assert.equal(
    requests[0].headers["x-open-compute-startup-generation"],
    "generation",
  );
  assert.equal(requests[0].headers["x-open-compute-internal-token"], undefined);
  await resolve(false, false, true);
  assert.equal(requests.at(-1).body.start, false);
  await resolve(false, true, false);
  assert.ok(requests.at(-1).url.endsWith("begin-stop"));
  assert.deepEqual(requests.at(-1).body, {
    instanceId: "instance",
    namespaceResourceId: "namespace",
    applicationName: "app",
    routeGeneration: 5,
    descriptorSha256: "d".repeat(64),
    objectId: "object",
    objectGeneration: 4,
    leaseGeneration: 2,
    opaqueEngineKey: lease.opaqueEngineKey,
  });
  assert.equal(await resolve(false, true, true), undefined);
  assert.ok(requests.at(-1).url.endsWith("/stop"));
  const count = requests.length;
  await resolve(false, true, true);
  assert.equal(requests.length, count);
});

test("attachment rejects class substitution and never masks private authority failures", async () => {
  const callbacks = [];
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_f, _c, resolve) {
        callbacks.push(
          (
            start,
            stop,
            completed,
            startup = "",
            expectedLeaseKey = "",
            monitor = false,
          ) =>
            resolve(start, stop, completed, startup, expectedLeaseKey, monitor),
        );
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch() {
        return new Response("private provider paths", { status: 503 });
      },
    },
  };
  for (const patch of [
    { className: "Other" },
    { namespaceResourceId: "other" },
  ])
    assert.throws(
      () =>
        containerAttachment(env, { facets: {} }, authority, {
          ...declaration,
          ...patch,
        }),
      /VERSION_INVARIANT_VIOLATION/,
    );
  containerAttachment(env, { facets: {} }, authority, declaration);
  await assert.rejects(
    callbacks[0](true, false, false),
    /^Error: CONTAINER_UNAVAILABLE$/,
  );
  env.RUNTIME_SOURCE.fetch = async () => Response.json({ lease: null });
  assert.equal(await callbacks[0](false, false, false), undefined);
  env.RUNTIME_SOURCE.fetch = async () =>
    Response.json({ lease: { ...lease, opaqueEngineKey: "broken" } });
  await assert.rejects(
    callbacks[0](true, false, false),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("normal operations require positive Running evidence while stop can clean Lost authority", async () => {
  let resolve;
  let state = "starting";
  let stopRequestedAtMs = null;
  const requests = [];
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_f, _c, callback) {
        resolve = (
          start,
          stop,
          completed,
          startup = "",
          expectedLeaseKey = "",
          monitor = false,
        ) =>
          callback(start, stop, completed, startup, expectedLeaseKey, monitor);
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url) {
        requests.push(url);
        return url.endsWith("resolve")
          ? Response.json({ lease: { ...lease, state, stopRequestedAtMs } })
          : new Response(null, { status: 204 });
      },
    },
  };
  containerAttachment(env, { facets: {} }, authority, declaration);
  assert.ok(await resolve(true, false, false));
  for (state of ["admitting", "starting", "lost", "stopping"])
    await assert.rejects(resolve(false, false, true), /CONTAINER_UNAVAILABLE/);
  for (state of ["admitting", "lost", "stopping"])
    await assert.rejects(resolve(true, false, false), /CONTAINER_UNAVAILABLE/);
  state = "running";
  assert.ok(await resolve(false, false, true));
  stopRequestedAtMs = 1;
  await assert.rejects(resolve(false, false, false), /CONTAINER_UNAVAILABLE/);
  await assert.rejects(resolve(true, false, false), /CONTAINER_UNAVAILABLE/);
  // A fresh stopped-path lookup accepts Lost state only to persist intent and prove cleanup.
  containerAttachment(env, { facets: {} }, authority, declaration);
  state = "lost";
  await resolve(false, true, false);
  assert.ok(requests.at(-1).endsWith("begin-stop"));
  assert.equal(await resolve(false, true, true), undefined);
  assert.ok(requests.at(-1).endsWith("/stop"));
});

test("old monitor cleanup cannot stop a replacement lease after destroy and restart", async () => {
  let resolve;
  let wireLease = lease;
  const requests = [];
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_f, _c, callback) {
        resolve = (
          start,
          stop,
          completed,
          startup = "",
          expectedLeaseKey = "",
          monitor = false,
        ) =>
          callback(start, stop, completed, startup, expectedLeaseKey, monitor);
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url) {
        requests.push(url);
        return url.endsWith("resolve")
          ? Response.json({ lease: wireLease })
          : new Response(null, { status: 204 });
      },
    },
  };
  containerAttachment(env, { facets: {} }, authority, declaration);
  await resolve(true, false, false);
  await resolve(false, true, false, "", lease.opaqueEngineKey);
  await resolve(false, true, true, "", lease.opaqueEngineKey);
  wireLease = { ...lease, opaqueEngineKey: "b".repeat(64), leaseGeneration: 3 };
  await resolve(true, false, false);
  const before = requests.length;
  await assert.rejects(
    resolve(false, true, false, "", lease.opaqueEngineKey),
    /CONTAINER_UNAVAILABLE/,
  );
  await assert.rejects(
    resolve(false, true, true, "", lease.opaqueEngineKey),
    /CONTAINER_UNAVAILABLE/,
  );
  assert.equal(requests.length, before);
  await assert.rejects(
    resolve(false, false, true, "", lease.opaqueEngineKey),
    /CONTAINER_UNAVAILABLE/,
  );
  const replacement = wireLease;
  wireLease = null;
  await assert.rejects(
    resolve(false, false, true, "", replacement.opaqueEngineKey),
    /CONTAINER_UNAVAILABLE/,
  );
  wireLease = replacement;
  await resolve(true, false, false);
  await resolve(false, true, false, "", wireLease.opaqueEngineKey);
  assert.ok(requests.at(-1).endsWith("begin-stop"));
  await assert.rejects(
    resolve(false, true, true, "", "bad"),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("native startup metadata preserves all execution overrides for encrypted lease authority", () => {
  assert.equal(containerStartupFromWire(""), undefined);
  assert.deepEqual(
    containerStartupFromWire(
      JSON.stringify({
        source: { image: `sha256:${"e".repeat(64)}` },
        instance: { custom: { vcpu: 0.5, memoryMib: "512", diskMb: "2000" } },
        enableInternet: true,
        environmentVariables: ["SECRET=private"],
        entrypoint: ["app"],
        labels: [{ name: "purpose", value: "test" }],
      }),
    ),
    {
      image: `sha256:${"e".repeat(64)}`,
      resources: { vcpu: 0.5, memoryMib: 512, diskMb: 2000 },
      enableInternet: true,
      entrypoint: ["app"],
      environmentVariables: ["SECRET=private"],
      labels: [{ name: "purpose", value: "test" }],
    },
  );
  assert.deepEqual(containerStartupFromWire('{"instance":{"named":"lite"}}'), {
    instanceProfile: "lite",
    enableInternet: false,
  });
  const snapshotId = "bb69566f-64d4-4b71-8dbe-311022d3c250";
  assert.deepEqual(
    containerStartupFromWire(
      JSON.stringify({ source: { containerSnapshotId: snapshotId } }),
    ),
    { containerSnapshotId: snapshotId, enableInternet: false },
  );
  assert.throws(
    () =>
      containerStartupFromWire(
        '{"directorySnapshots":[{"snapshotId":"bad","restorePath":"/a"}]}',
      ),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.deepEqual(
    containerStartupFromWire(
      JSON.stringify({
        directorySnapshots: [
          { snapshotId, restorePath: "/saved" },
          { restorePath: "//empty/./data/../" },
        ],
      }),
    ),
    {
      enableInternet: false,
      directorySnapshots: [
        { snapshotId, mountPoint: "/saved" },
        { snapshotId: null, mountPoint: "/empty" },
      ],
    },
  );
  assert.deepEqual(containerStartupFromWire('{"directorySnapshots":[]}'), {
    enableInternet: false,
    directorySnapshots: [],
  });
  for (const entry of [
    null,
    [],
    {},
    { restorePath: "/" },
    { restorePath: "/../a" },
    { restorePath: "relative" },
    { restorePath: "/a\0b" },
    { restorePath: `/${"界".repeat(1366)}` },
    { restorePath: "/a", snapshotId: "" },
    { restorePath: "/a", snapshotId: null },
    { restorePath: "/a", snapshotId: 12 },
  ]) {
    assert.throws(
      () =>
        containerStartupFromWire(
          JSON.stringify({ directorySnapshots: [entry] }),
        ),
      /VERSION_INVARIANT_VIOLATION/,
    );
  }
  for (const value of [
    '{"entrypoint":{}}',
    '{"entrypoint":[1]}',
    '{"labels":{}}',
    '{"labels":[{}]}',
    "{",
    "[]",
    '{"enableInternet":1}',
    '{"source":[]}',
    '{"source":{"image":""}}',
    '{"source":{"containerSnapshotId":"bad"}}',
    JSON.stringify({ source: { containerSnapshotId: "f".repeat(64) } }),
    '{"instance":[]}',
    '{"instance":{"named":1}}',
    '{"instance":{}}',
    '{"instance":{"custom":{"vcpu":0}}}',
    '{"instance":{"custom":{"vcpu":1,"memoryMib":"bad","diskMb":2}}}',
    '{"directorySnapshots":{}}',
  ])
    assert.throws(
      () => containerStartupFromWire(value),
      /VERSION_INVARIANT_VIOLATION/,
    );
});

test("monitor lookup preserves exact exited lease while ordinary operations require Running", async () => {
  let resolve;
  let current = { ...lease };
  const requests = [];
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_facets, _class, callback) {
        resolve = callback;
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url, init) {
        requests.push({ url, body: JSON.parse(init.body) });
        return url.endsWith("resolve")
          ? Response.json({ lease: current })
          : new Response(null, { status: 204 });
      },
    },
  };
  containerAttachment(env, { facets: {} }, authority, declaration);
  await resolve(true, false, false, "", "", false);
  await resolve(false, false, true, "", lease.opaqueEngineKey, false);
  current = { ...lease, state: "lost" };
  const monitored = await resolve(
    false,
    false,
    false,
    "",
    lease.opaqueEngineKey,
    true,
  );
  assert.equal(monitored.leaseKey, lease.opaqueEngineKey);
  assert.equal(requests.at(-1).body.forMonitor, true);
  await assert.rejects(
    resolve(false, false, false, "", lease.opaqueEngineKey, false),
    /CONTAINER_UNAVAILABLE/,
  );
  for (const patch of [
    { opaqueEngineKey: "b".repeat(64), leaseGeneration: 3 },
    { state: "admitting" },
    { state: "stopping" },
    { stopRequestedAtMs: 1 },
  ]) {
    current = { ...lease, ...patch };
    await assert.rejects(
      resolve(false, false, false, "", lease.opaqueEngineKey, true),
      /CONTAINER_UNAVAILABLE/,
    );
  }
  current = null;
  await assert.rejects(
    resolve(false, false, false, "", lease.opaqueEngineKey, true),
    /CONTAINER_UNAVAILABLE/,
  );
  for (const args of [
    [true, false, false, "", "", true],
    [false, true, false, "", "", true],
    [false, false, true, "", "", true],
    [false, false, false, "{}", "", true],
  ])
    await assert.rejects(resolve(...args), /VERSION_INVARIANT_VIOLATION/);
});

test("directory snapshot metadata binds the exact canonical capture path", () => {
  const snapshot = {
    id: "directory-snapshot",
    size: 123,
    dir: "/app/data",
    name: "checkpoint",
  };
  assert.deepEqual(
    containerSnapshotFromWire({ snapshot }, "/app/data"),
    snapshot,
  );
  for (const dir of [
    undefined,
    null,
    1,
    "/different",
    "//app/data",
    "/app/data/.",
  ]) {
    const value = { ...snapshot, dir };
    if (dir === undefined) delete value.dir;
    assert.throws(
      () => containerSnapshotFromWire({ snapshot: value }, "/app/data"),
      /VERSION_INVARIANT_VIOLATION/,
    );
  }
  assert.throws(
    () => containerSnapshotFromWire({ snapshot }),
    /VERSION_INVARIANT_VIOLATION/,
  );
  assert.throws(
    () =>
      containerSnapshotFromWire(
        { snapshot: { ...snapshot, enginePath: "/private" } },
        "/app/data",
      ),
    /VERSION_INVARIANT_VIOLATION/,
  );
});

test("directory snapshot capture keeps current lease fencing and normalizes only API input", async () => {
  let capture;
  let current = lease;
  let metadata = (body) => ({
    id: "directory-snapshot",
    size: 123,
    dir: body.directory,
    ...(body.name === undefined ? {} : { name: body.name }),
  });
  const requests = [];
  const env = {
    WORKER_LOADER_FACTORY: {
      getContainer(_facets, _class, _resolve, callback) {
        capture = callback;
        return {};
      },
    },
    RUNTIME_SOURCE: {
      async fetch(url, init) {
        const body = JSON.parse(init.body);
        requests.push({ url, body });
        return Response.json(
          url.endsWith("resolve")
            ? { lease: current }
            : { snapshot: metadata(body) },
        );
      },
    },
  };
  const mutable = { ...authority };
  containerAttachment(env, { facets: {} }, mutable, declaration);
  mutable.routeGeneration = 99;
  assert.deepEqual(
    await capture(
      lease.opaqueEngineKey,
      "checkpoint",
      "//app/./state/../data/",
    ),
    {
      id: "directory-snapshot",
      size: 123,
      dir: "/app/data",
      name: "checkpoint",
    },
  );
  const saved = requests.at(-1).body;
  assert.equal(saved.directory, "/app/data");
  assert.equal(saved.source.routeGeneration, authority.routeGeneration);
  assert.equal(saved.source.opaqueEngineKey, lease.opaqueEngineKey);
  assert.equal(saved.source.leaseGeneration, lease.leaseGeneration);
  assert.deepEqual(await capture(lease.opaqueEngineKey, undefined, "/"), {
    id: "directory-snapshot",
    size: 123,
    dir: "/",
  });
  for (const directory of [
    null,
    1,
    "",
    "relative",
    "/../outside",
    "/app/../../outside",
    "/nul\0path",
    "/" + "界".repeat(1366),
  ]) {
    const before = requests.length;
    await assert.rejects(
      capture(lease.opaqueEngineKey, undefined, directory),
      /VERSION_INVARIANT_VIOLATION/,
    );
    assert.equal(requests.length, before);
  }
  for (current of [
    null,
    { ...lease, state: "lost" },
    { ...lease, opaqueEngineKey: "b".repeat(64) },
    { ...lease, stopRequestedAtMs: 1 },
  ]) {
    const before = requests.filter(({ url }) =>
      url.endsWith("snapshot"),
    ).length;
    await assert.rejects(
      capture(lease.opaqueEngineKey, undefined, "/app/data"),
      /CONTAINER_UNAVAILABLE/,
    );
    assert.equal(
      requests.filter(({ url }) => url.endsWith("snapshot")).length,
      before,
    );
  }
  current = lease;
  for (const reply of [
    { id: "wrong-kind", size: 1 },
    { id: "wrong-directory", size: 1, dir: "/other" },
  ]) {
    metadata = () => reply;
    await assert.rejects(
      capture(lease.opaqueEngineKey, undefined, "/app/data"),
      /VERSION_INVARIANT_VIOLATION/,
    );
  }
});
