import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import test from "node:test";
import { setImmediate } from "node:timers/promises";
import { compileRuntime } from "../../../packages/runtime/tests/compiled-runtime.mjs";

const url = (code) =>
  `data:text/javascript;base64,${Buffer.from(code).toString("base64")}`;
function failureDetails(body, error) {
  const message = error instanceof Error ? error.message : "";
  const bytes = new TextEncoder().encode(message.slice(0, 4096));
  const {
    nativeErrorMessageBytes,
    nativeErrorMessageTruncated,
    nativeErrorMessageSha256,
    nativeErrorSites,
    nativeErrorInternalReference,
    ...details
  } = body;
  assert.equal(nativeErrorMessageBytes, bytes.byteLength);
  assert.equal(nativeErrorMessageTruncated, message.length > 4096);
  assert.equal(
    nativeErrorMessageSha256,
    createHash("sha256").update(bytes).digest("hex"),
  );
  assert.equal(
    nativeErrorInternalReference,
    message.startsWith("internal error"),
  );
  assert.ok(Array.isArray(nativeErrorSites));
  assert.ok(
    nativeErrorSites.every((site) =>
      [
        "createStubPolicy",
        "StubPolicy.fetch",
        "NamespacePolicy.get",
        "makeId",
        "dispatchFetch",
        "routeFetch",
        "DoHost",
        "FixtureContainer",
        "NamedImageContainer",
      ].includes(site),
    ),
  );
  return details;
}

const failureDiagnosticCode = await compileRuntime(
  "../../../test/applications/containers/src/failure-diagnostic.ts",
);
const { readinessDiagnostic } = await import(url(failureDiagnosticCode));

const directoryCode = await compileRuntime(
  "../../../test/applications/containers/src/directory-snapshots.ts",
);
const { directoryAction, directoryHandle } = await import(url(directoryCode));
let code = await compileRuntime(
  "../../../test/applications/containers/src/named-images.ts",
);
for (const [name, replacement] of Object.entries({
  "./directory-snapshots": directoryCode,
  "./failure-diagnostic": failureDiagnosticCode,
  "cloudflare:workers":
    "export class DurableObject { constructor(ctx) { this.ctx = ctx; } }",
  "./exec-options-lifecycle":
    "export const execOptionsLifecycle = () => { throw new Error('unexpected exec'); };",
  "./exec-users-lifecycle":
    "export const execUsersLifecycle = () => { throw new Error('unexpected exec user probe'); };",
  "./signals-lifecycle":
    "export const signalsLifecycle = () => { throw new Error('unexpected signals'); };",
})) {
  assert.ok(code.includes(JSON.stringify(name)), name);
  code = code.replaceAll(
    JSON.stringify(name),
    JSON.stringify(url(replacement)),
  );
}
const { NamedImageContainer } = await import(url(code));

test("readiness records the final HTTP status instead of an earlier exception", async (t) => {
  let clock = 0;
  t.mock.method(Date, "now", () => (++clock <= 3 ? 0 : 20000));
  let attempts = 0;
  const native = {
    ...probeNative(),
    images: { base: "base", alternate: "alternate" },
    getTcpPort() {
      return {
        async fetch() {
          if (++attempts === 1) throw new Error("private earlier failure");
          return new Response("", { status: 503 });
        },
      };
    },
  };
  const response = await new NamedImageContainer({ container: native }).fetch(
    new Request("http://fixture/named/snapshot-write?value=mutated"),
  );
  assert.equal(response.status, 500);
  const body = await response.json();
  assert.equal(body.readiness.attempts, 2);
  assert.equal(body.readiness.lastStatus, 503);
  assert.equal(body.readiness.lastFailure, null);
  assert.ok(!JSON.stringify(body).includes("private earlier failure"));
});

const loopbackState = {
  pidState: "S",
  listening: 1,
  established: 0,
  closeWait: 0,
  httpStatus: 200,
  httpError: null,
  httpBytes: 4,
};
function probeNative(
  stdout = JSON.stringify(loopbackState),
  stderr = "",
  exitCode = 0,
) {
  return {
    running: true,
    async exec(command, options) {
      assert.deepEqual(command.slice(0, 2), ["python3", "-c"]);
      assert.ok(options.signal instanceof AbortSignal);
      assert.equal(options.stdout, "pipe");
      assert.equal(options.stderr, "pipe");
      return {
        async output() {
          return {
            exitCode,
            stdout: new TextEncoder().encode(stdout).buffer,
            stderr: new TextEncoder().encode(stderr).buffer,
          };
        },
      };
    },
  };
}

test("readiness diagnostics distinguish the last timeout from container loopback health", async () => {
  const error = new DOMException("private timeout detail", "TimeoutError");
  const native = probeNative();
  const execute = native.exec;
  native.exec = async (command, options) => {
    execFileSync(
      "python3",
      ["-B", "-c", "import sys; compile(sys.stdin.read(), '<probe>', 'exec')"],
      { input: command[2] },
    );
    return execute(command, options);
  };
  const value = await readinessDiagnostic(native, 20, null, error);
  assert.equal(value.attempts, 20);
  assert.equal(value.lastStatus, null);
  assert.equal(value.nativeRunning, true);
  assert.equal(value.lastFailure.nativeErrorCode, "REQUEST_TIMEOUT");
  failureDetails(value.lastFailure, error);
  for (const [key, expected] of Object.entries(loopbackState))
    assert.equal(value.loopback[key], expected);
  assert.ok(!JSON.stringify(value).includes("private timeout detail"));
});

test("readiness probe projection rejects extra fields, invalid types and private output", async () => {
  for (const [stdout, stderr, exitCode] of [
    [JSON.stringify({ ...loopbackState, token: "private-token" }), "", 0],
    [JSON.stringify({ ...loopbackState, httpError: ["TimeoutError"] }), "", 0],
    [JSON.stringify({ ...loopbackState, listening: -2 }), "", 0],
    [JSON.stringify({ ...loopbackState, httpBytes: 4097 }), "", 0],
    [JSON.stringify({ ...loopbackState, httpError: "private-token" }), "", 0],
    ["private-token".repeat(1000), "", 0],
    ["", "private-token", 1],
  ]) {
    const value = await readinessDiagnostic(
      probeNative(stdout, stderr, exitCode),
      3,
      503,
      undefined,
    );
    assert.equal(value.lastStatus, 503);
    assert.equal(value.lastFailure, null);
    assert.ok(!JSON.stringify(value).includes("private-token"));
    assert.equal(Object.hasOwn(value.loopback, "httpStatus"), false);
  }
});

test("readiness probe failure keeps its bounded error fingerprint", async () => {
  const error = new TypeError("private exec error");
  const value = await readinessDiagnostic(
    {
      running: true,
      async exec() {
        throw error;
      },
    },
    1,
    null,
    error,
  );
  failureDetails(value.loopback.failure, error);
  assert.equal(value.loopback.failure.nativeErrorClass, "TypeError");
  assert.ok(!JSON.stringify(value).includes("private exec error"));
});

test("named readiness still fails at its original bound even when diagnostic loopback succeeds", async (t) => {
  let calls = 0;
  t.mock.method(Date, "now", () => (++calls <= 2 ? 0 : 20000));
  let attempts = 0;
  const native = {
    ...probeNative(),
    images: { base: "base", alternate: "alternate" },
    getTcpPort() {
      return {
        async fetch() {
          attempts++;
          throw new Error("Network connection lost.");
        },
      };
    },
  };
  const object = new NamedImageContainer({ container: native });
  const response = await object.fetch(
    new Request("http://fixture/named/snapshot-write?value=mutated"),
  );
  assert.equal(response.status, 500);
  assert.equal(attempts, 1);
  const body = await response.json();
  assert.equal(body.phase, "named-readiness");
  assert.equal(body.readiness.attempts, 1);
  assert.equal(
    body.readiness.lastFailure.nativeErrorCode,
    "NETWORK_CONNECTION_LOST",
  );
  assert.equal(body.readiness.loopback.httpStatus, 200);
});

test("directory fixture rejects malformed handles and preserves captured metadata", () => {
  const handle = {
    id: "snapshot",
    size: 8192,
    dir: "/tmp/state",
    name: "saved",
  };
  assert.deepEqual(directoryHandle(handle), handle);
  for (const value of [
    null,
    [],
    {},
    { ...handle, id: "" },
    { ...handle, size: -1 },
    { ...handle, size: 0.1 },
    { ...handle, size: Number.MAX_SAFE_INTEGER + 1 },
    { ...handle, dir: "relative" },
    { ...handle, dir: "/nul\0path" },
    { ...handle, name: 1 },
    { ...handle, unexpected: "field" },
  ]) {
    assert.throws(() => directoryHandle(value), {
      message: "Invalid directory fixture handle",
    });
  }
});

test("directory fixture forwards capture and restore parameters for both policies", async () => {
  const handle = {
    id: "captured",
    size: 8192,
    dir: "/tmp/p23-directory-state",
  };
  let captured;
  const capturing = {
    running: true,
    async snapshotDirectory(options) {
      captured = options;
      return handle;
    },
  };
  const capture = await directoryAction(
    capturing,
    "declared-image",
    new Request("http://fixture/named/directory/capture?unnamed=1"),
  );
  assert.deepEqual(await capture.json(), handle);
  assert.deepEqual(captured, { dir: "//tmp/./p23-directory-state/" });
  await assert.rejects(
    directoryAction(
      { running: true },
      undefined,
      new Request("http://fixture/named/directory/capture"),
    ),
    { message: "Native directory capture is required" },
  );
  for (const image of [undefined, "declared-image"]) {
    let startup;
    const native = {
      running: false,
      start(options) {
        startup = options;
        this.running = true;
      },
      async exec(command, options) {
        assert.deepEqual(command.slice(0, 2), ["python3", "-c"]);
        assert.deepEqual(options, { user: "0:0", cwd: "/" });
        return {
          async output() {
            return {
              exitCode: 0,
              stdout: new TextEncoder().encode('{"read":true}').buffer,
              stderr: new ArrayBuffer(0),
            };
          },
        };
      },
    };
    const request = () =>
      new Request("http://fixture/named/directory/restore", {
        method: "POST",
        body: JSON.stringify(handle),
      });
    const response = await directoryAction(native, image, request());
    assert.deepEqual(await response.json(), { read: true });
    assert.deepEqual(startup, {
      ...(image === undefined ? {} : { image, instance: "lite" }),
      enableInternet: false,
      directorySnapshots: [
        { snapshot: handle },
        { snapshot: handle, mountPoint: "/tmp/p23-directory-copy" },
        { mountPoint: "/tmp/p23-directory-empty" },
        { mountPoint: "/tmp/p23-directory-copy/child" },
      ],
    });
    await assert.rejects(directoryAction(native, image, request()), {
      message: "Directory restore requires a stopped fixture",
    });
  }
});

test("directory replay requires the running start rejection and preserves directory reads", async () => {
  const handle = {
    id: "captured",
    size: 8192,
    dir: "/tmp/p23-directory-state",
  };
  const tree = { source: "mutated", copy: "saved", empty: [] };
  const request = () =>
    new Request("http://fixture/named/directory/replay", {
      method: "POST",
      body: JSON.stringify(handle),
    });
  for (const rejected of [true, false]) {
    let starts = 0;
    let reads = 0;
    const native = {
      running: true,
      start() {
        starts++;
        if (rejected) throw new Error("Container is already running");
      },
      async exec() {
        reads++;
        return {
          async output() {
            return {
              exitCode: 0,
              stdout: new TextEncoder().encode(JSON.stringify(tree)).buffer,
              stderr: new ArrayBuffer(0),
            };
          },
        };
      },
    };
    if (rejected) {
      const response = await directoryAction(
        native,
        "declared-image",
        request(),
      );
      assert.deepEqual(await response.json(), { ...tree, startRejected: true });
      assert.equal(reads, 1);
    } else {
      await assert.rejects(
        directoryAction(native, "declared-image", request()),
        {
          message: "Running fixture start must reject",
        },
      );
      assert.equal(reads, 0);
    }
    assert.equal(starts, 1);
    native.running = false;
    await assert.rejects(directoryAction(native, "declared-image", request()), {
      message: "Directory replay requires a running fixture",
    });
    assert.equal(starts, 1);
  }
});

function fixture(snapshotContainer, path = "/named/snapshot") {
  const native = {
    running: true,
    images: { base: "base", alternate: "alternate" },
    getTcpPort: (port) => {
      assert.equal(port, 8080);
      return { fetch: async () => new Response("ready") };
    },
    snapshotContainer,
  };
  const phases = [];
  const object = new NamedImageContainer({
    container: native,
    storage: {
      sql: {
        exec(query, phase, value) {
          if (query.startsWith("INSERT INTO snapshot_diagnostic_phase"))
            phases.push({ phase, value });
        },
      },
    },
  });
  const response = object.fetch(new Request(`http://fixture${path}`));
  response.phases = phases;
  return response;
}

test("default snapshot capture calls the native method with no arguments", async () => {
  let calls = 0;
  const handle = { id: "default-snapshot", size: 123 };
  const response = await fixture((...args) => {
    assert.equal(args.length, 0);
    calls++;
    return Promise.resolve(handle);
  }, "/named/snapshot-default");
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), handle);
  assert.equal(calls, 1);
});

test("snapshot response headers precede completion, but its JSON waits for the real handle", async () => {
  let complete;
  const pending = new Promise((resolve) => {
    complete = resolve;
  });
  let calls = 0;
  const responsePromise = fixture((options) => {
    assert.deepEqual(options, { name: "fixture-saved" });
    calls++;
    return pending;
  });
  const response = await Promise.race([
    responsePromise,
    setImmediate().then(() => null),
  ]);
  if (response === null) {
    complete({ id: "cleanup", size: 1 });
    await responsePromise;
    assert.fail("snapshot completion withheld response headers");
  }
  assert.deepEqual(responsePromise.phases, [
    { phase: "CAPTURE_REQUESTED", value: undefined },
  ]);
  assert.equal(response.status, 200);
  assert.equal(response.headers.get("content-type"), "application/json");
  let settled = false;
  const body = response.json().finally(() => {
    settled = true;
  });
  await setImmediate();
  assert.equal(
    settled,
    false,
    "a pending snapshot must not produce a successful handle",
  );
  const handle = { id: "completed-snapshot", name: "fixture-saved", size: 123 };
  complete(handle);
  assert.deepEqual(await body, handle);
  assert.equal(calls, 1);
  assert.deepEqual(responsePromise.phases, [
    { phase: "CAPTURE_REQUESTED", value: undefined },
    {
      phase: "CAPTURE_RETURNED",
      value: new TextEncoder().encode(JSON.stringify(handle)).byteLength,
    },
  ]);
});

for (const synchronous of [false, true]) {
  test(`snapshot ${synchronous ? "synchronous" : "asynchronous"} failure rejects its body without exposing the exception`, async () => {
    const responsePromise = fixture(() => {
      const error = new Error("private snapshot exception");
      if (synchronous) throw error;
      return Promise.reject(error);
    });
    const response = await responsePromise;
    await assert.rejects(response.json(), {
      message: "snapshot fixture capture failed",
    });
    assert.deepEqual(responsePromise.phases, [
      { phase: "CAPTURE_REQUESTED", value: undefined },
      { phase: "CAPTURE_REJECTED", value: null },
    ]);
  });
}

let mainCode = await compileRuntime(
  "../../../test/applications/containers/src/index.ts",
);
for (const [name, replacement] of Object.entries({
  "./failure-diagnostic": failureDiagnosticCode,
  "@cloudflare/containers":
    "export class Container { constructor(ctx,env) { this.ctx=ctx; this.env=env; } async startAndWaitForPorts() {} } export class ContainerProxy {}",
  "./exec-lifecycle": "export const execLifecycle = () => {};",
  "./exec-users-lifecycle": "export const execUsersLifecycle = () => {};",
  "./inactivity-lifecycle": "export class RawIdleContainer {}",
  "./named-images": "export class NamedImageContainer {}",
  "./network-lifecycle": "export const networkLifecycle = () => {};",
})) {
  assert.ok(mainCode.includes(JSON.stringify(name)), name);
  mainCode = mainCode.replaceAll(
    JSON.stringify(name),
    JSON.stringify(url(replacement)),
  );
}
const { FixtureContainer } = await import(url(mainCode));

for (const [message, category] of [
  ["Container operation failed.", "CONTAINER_OPERATION_FAILED"],
  ["Network connection lost.", "NETWORK_CONNECTION_LOST"],
  ["The Container is not running.", "CONTAINER_NOT_RUNNING"],
  ["Container execution authority changed.", "CONTAINER_AUTHORITY_CHANGED"],
  ["Container port connection closed unexpectedly.", "CONTAINER_PORT_CLOSED"],
  ["Container port connection read aborted.", "CONTAINER_PORT_READ_ABORTED"],
  ["private unexpected error with credentials and topology", null],
]) {
  test(`main HTTP diagnostic exposes only fixed category ${category}`, async () => {
    const object = new FixtureContainer(
      {
        id: { equals: () => false },
        storage: { sql: { exec() {} } },
        container: {
          getTcpPort: () => ({
            fetch: async () => {
              throw new Error(message);
            },
          }),
        },
      },
      { OBJECTS: { idFromName: () => null } },
    );
    const response = await object.fetch(new Request("http://fixture/"));
    assert.equal(response.status, 500);
    const body = await response.json();
    failureDetails(body, new Error(message));
    assert.equal(body.phase, "main-http");
    assert.equal(body.nativeErrorCode, category);
    assert.ok(!JSON.stringify(body).includes(message));
  });
}

test("HTTP failure fingerprints bound messages and expose only fixed stack sites", async () => {
  for (const message of [
    "private/token/detail",
    "internal error; private detail",
    "秘密".repeat(5000),
  ]) {
    const error = new TypeError(message);
    error.stack = `TypeError: ${message}\n at StubPolicy.fetch (https://private.invalid/token:1:2)\n at privateFunction (/private/path:3:4)`;
    const object = new FixtureContainer(
      {
        id: { equals: () => false },
        storage: { sql: { exec() {} } },
        container: {
          getTcpPort: () => ({
            fetch: async () => {
              throw error;
            },
          }),
        },
      },
      { OBJECTS: { idFromName: () => null } },
    );
    const response = await object.fetch(new Request("http://fixture/"));
    const body = await response.json();
    assert.equal(response.status, 500);
    failureDetails(body, error);
    assert.deepEqual(body.nativeErrorSites, ["StubPolicy.fetch"]);
    for (const secret of [
      message,
      "private.invalid",
      "/private/path",
      "privateFunction",
    ])
      assert.ok(!JSON.stringify(body).includes(secret));
  }
});

test("named dispatch diagnostics preserve stable codes without upstream details", async () => {
  const imports = {
    "./failure-diagnostic": failureDiagnosticCode,
    "@cloudflare/containers":
      "export class Container { constructor(ctx, env) { this.ctx = ctx; this.env = env; } async startAndWaitForPorts() {} } export class ContainerProxy {}",
    "./exec-lifecycle": "export const execLifecycle = () => {};",
    "./exec-users-lifecycle": "export const execUsersLifecycle = () => {};",
    "./inactivity-lifecycle": "export class RawIdleContainer {}",
    "./named-images": "export class NamedImageContainer {}",
    "./network-lifecycle": "export const networkLifecycle = () => {};",
  };
  const { default: fixture, FixtureContainer } = await import(
    url(
      await compileRuntime(
        "../../../test/applications/containers/src/index.ts",
        Object.fromEntries(
          Object.entries(imports).map(([name, source]) => [name, url(source)]),
        ),
      ),
    )
  );
  for (const code of [
    "DO_VERSION_STALE",
    "DO_OBJECT_DELETING",
    "DO_STORAGE_UNAVAILABLE",
    "VERSION_INVARIANT_VIOLATION",
  ]) {
    for (const point of ["getByName", "fetch"]) {
      const response = await fixture.fetch(
        new Request("http://fixture/named/directory/surface"),
        {
          NAMED_OBJECTS: {
            getByName() {
              if (point === "getByName") throw new Error(code);
              return {
                async fetch() {
                  throw new Error(code);
                },
              };
            },
          },
        },
      );
      assert.equal(response.status, 500);
      assert.deepEqual(failureDetails(await response.json(), new Error(code)), {
        ok: false,
        diagnostic: "controlled-fixture",
        phase: "named-object-dispatch-or-construction",
        nativeErrorCode: code,
        nativeErrorClass: "Error",
      });
    }
  }
  for (const [fault, kind] of [
    [new Error("private/token/detail"), "Error"],
    [new TypeError("private/raw/detail"), "TypeError"],
    ["private/thrown/detail", "Unknown"],
  ]) {
    const response = await fixture.fetch(
      new Request("http://fixture/named/directory/surface"),
      {
        NAMED_OBJECTS: {
          getByName() {
            throw fault;
          },
        },
      },
    );
    assert.equal(response.status, 500);
    assert.deepEqual(failureDetails(await response.json(), fault), {
      ok: false,
      diagnostic: "controlled-fixture",
      phase: "named-object-dispatch-or-construction",
      nativeErrorCode: null,
      nativeErrorClass: kind,
    });
  }
  const encode = (text) => new TextEncoder().encode(text).buffer;
  for (const [stderr, stdout, exitCode, expected] of [
    [
      "egress-status=200;code-present=1;body-bytes=3\n",
      "one\n",
      0,
      [200, "1", 3],
    ],
    ["egress-status=200;code-present=0;body-bytes=0\n", "\n", 0, [200, "0", 0]],
    [
      "egress-status=500;code-present=missing;body-bytes=0\n",
      "\n",
      1,
      [500, "missing", 0],
    ],
    ["private/path/token/error", "", 1, [null, null, null]],
  ]) {
    const recorded = [];
    const output = (text, error = "", code = 0) => ({
      stdout: encode(text),
      stderr: encode(error),
      exitCode: code,
    });
    const outputs = [
      output("exec-ok"),
      output("explicit-ok"),
      output(stdout, stderr, exitCode),
    ];
    const native = {
      running: true,
      getTcpPort: () => ({ fetch: async () => new Response("one") }),
      exec: async () => ({ output: async () => outputs.shift() }),
    };
    const object = new FixtureContainer(
      {
        id: { equals: () => false },
        container: native,
        storage: {
          sql: {
            exec(query, ...values) {
              if (query.startsWith("INSERT INTO fixture_egress_diagnostic"))
                recorded.push(values);
              return { one: () => ({ count: 2 }) };
            },
          },
        },
      },
      { CODE_REVISION: "one", OBJECTS: { idFromName: () => null } },
    );
    const response = await object.fetch(new Request("http://fixture/"));
    assert.equal(response.status, 200);
    const body = await response.json();
    assert.equal(body.egress, stdout.trim());
    assert.equal(body.egressExitCode, exitCode);
    assert.deepEqual(recorded, [
      [
        ...expected,
        encode(stdout).byteLength,
        encode(stderr).byteLength,
        exitCode,
      ],
    ]);
    assert.ok(!JSON.stringify(recorded).includes("private/path/token/error"));
    assert.equal(outputs.length, 0);
  }
});
