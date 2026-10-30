import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { runInNewContext } from "node:vm";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const source = (path) => readFileSync(resolve(root, path), "utf8");
const sharedUrl = moduleUrl(
  await compileRuntime("loader/shared.ts", {
    "./snapshot.js": moduleUrl("export function assertSnapshot() {}"),
    "./python-snapshot.js": moduleUrl(
      'export function resolvePythonSnapshot() { throw Error("unexpected prepared snapshot read"); }',
    ),
  }),
);
const { tenantGlobalOutbound } = await import(sharedUrl);

function echoSockets({ denyPrivate = false, corruptPrivate = false } = {}) {
  const exchanges = [];
  function connect({ hostname }) {
    const privatePeer = hostname === "private.fixture";
    if (denyPrivate && privatePeer)
      return {
        opened: Promise.reject(new Error("fixture connection denied")),
        close: async () => {},
      };
    let controller;
    let ended = false;
    const chunks = [];
    return {
      opened: Promise.resolve(),
      closed: Promise.resolve(),
      readable: new ReadableStream({
        start(value) {
          controller = value;
        },
      }),
      writable: new WritableStream({
        write(chunk) {
          chunks.push(Buffer.from(chunk));
        },
        close() {
          const written = Buffer.concat(chunks);
          const newline = written.indexOf(10);
          const payload = Buffer.from(written.subarray(newline + 1));
          assert.equal(
            written.subarray(0, newline).toString(),
            `ECHO ${payload.length}`,
          );
          exchanges.push({ hostname, payload: payload.toString("hex") });
          if (corruptPrivate && privatePeer) payload[0] ^= 1;
          controller.enqueue(payload);
          controller.close();
          ended = true;
        },
      }),
      async close() {
        if (!ended) controller.close();
        ended = true;
      },
    };
  }
  return { connect, exchanges };
}

for (const [kind, path] of [
  [
    "Durable Object",
    "crates/service/tests/p0_8_scheduler_do_alarms_gate/mod.rs",
  ],
  ["Workflow", "crates/service/tests/workflow_runtime_gate/mod.rs"],
]) {
  function probe(connect) {
    const fixture = source(path);
    const start = fixture.indexOf("async function rawTcpProbe(");
    const end = fixture.indexOf("\nfunction ", start);
    assert.ok(start >= 0 && end > start);
    const run = runInNewContext(`(${fixture.slice(start, end)})`, {
      connect,
      Uint8Array,
      TextEncoder,
      Response,
    });
    const config = {
      hostname: "public.fixture",
      privateHostname: "private.fixture",
      tcpPort: 38081,
    };
    return () =>
      run(
        kind === "Durable Object"
          ? { RAW_TCP_CONFIG_JSON: JSON.stringify(config) }
          : config,
      );
  }
  test(`${kind} fixture requires public and private IP echo under the shared outbound policy`, async () => {
    const sockets = echoSockets();
    const result = await probe(sockets.connect)();
    assert.deepEqual(
      JSON.parse(JSON.stringify(result)),
      kind === "Durable Object" ? true : { bytes: 4, privateBytes: 4 },
    );
    assert.deepEqual(
      sockets.exchanges.map(({ hostname }) => hostname),
      ["public.fixture", "private.fixture"],
    );
    assert.equal(sockets.exchanges[0].payload, sockets.exchanges[1].payload);
  });
  for (const fault of ["denyPrivate", "corruptPrivate"]) {
    test(`${kind} fixture rejects ${fault} rather than accepting an obsolete private-denial policy`, async () => {
      await assert.rejects(probe(echoSockets({ [fault]: true }).connect));
    });
  }
}

test("tenant outbound selects one host-only capability and validation stays offline", () => {
  const network = Object.freeze({ fetch() {}, connect() {} });
  assert.equal(
    tenantGlobalOutbound({ OUTBOUND_NETWORK: network }, false),
    network,
  );
  assert.equal(tenantGlobalOutbound({}, true), null);
  assert.throws(
    () => tenantGlobalOutbound({}, false),
    (error) => error?.stableCode === "VERSION_INVARIANT_VIOLATION",
  );
});

test("every dynamic event source uses OUTBOUND_NETWORK and the HTTP-only gateway is gone", () => {
  const config = source("packages/runtime/config.capnp");
  for (const name of ["config", "prepareConfig"]) {
    const profile = config.match(
      new RegExp(
        `const ${name} :Workerd\\.Config = ([\\s\\S]*?)(?=\\nconst |$)`,
      ),
    )?.[1];
    assert.ok(profile, `${name} profile must exist`);
    assert.equal(
      (profile.match(/name = "outbound-network", network/g) ?? []).length,
      1,
      `${name} must own exactly one outbound network capability`,
    );
  }
  assert.match(
    config,
    /allow = \["network", "local"\][\s\S]*deny = \["unix", "unix-abstract"\][\s\S]*tlsOptions = \(trustBrowserCas = true\)/,
  );
  assert.equal(
    (
      config.match(
        /name = "OUTBOUND_NETWORK", service = "outbound-network"/g,
      ) ?? []
    ).length,
    2,
  );
  assert.doesNotMatch(config, /outbound-gateway|gateway\/outbound/);

  const owners = [
    "packages/runtime/src/loader/dispatch.ts",
    "packages/runtime/src/services/transport.ts",
    "packages/runtime/src/durable-objects/host.ts",
    "packages/runtime/src/workflows/host.ts",
  ]
    .map(source)
    .join("\n");
  assert.doesNotMatch(owners, /OutboundGateway|ctx\.exports\.Outbound/);
  assert.equal(
    (owners.match(/globalOutbound: tenantGlobalOutbound\(/g) ?? []).length,
    5,
  );
  assert.match(
    source("packages/runtime/src/loader/dispatch.ts"),
    /globalOutbound: null/,
  );
  assert.doesNotMatch(
    source("packages/runtime/src/loader/bindings.ts"),
    /OUTBOUND_NETWORK/,
  );
  assert.equal(
    existsSync(resolve(root, "packages/runtime/src/gateway/outbound.ts")),
    false,
  );
});
