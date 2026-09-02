import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../../../..");
const source = path => readFileSync(resolve(root, path), "utf8");
const sharedUrl = moduleUrl(await compileRuntime("loader/shared.ts", {
  "./snapshot.js": moduleUrl("export function assertSnapshot() {}"),
}));
const { tenantGlobalOutbound } = await import(sharedUrl);

test("tenant outbound selects one host-only capability and validation stays offline", () => {
  const network = Object.freeze({ fetch() {}, connect() {} });
  assert.equal(tenantGlobalOutbound({ PUBLIC_NETWORK: network }, false), network);
  assert.equal(tenantGlobalOutbound({}, true), null);
  assert.throws(
    () => tenantGlobalOutbound({}, false),
    error => error?.stableCode === "VERSION_INVARIANT_VIOLATION",
  );
});

test("every dynamic event source uses PUBLIC_NETWORK and the HTTP-only gateway is gone", () => {
  const config = source("packages/runtime/config.capnp");
  assert.equal((config.match(/name = "internet", network/g) ?? []).length, 1);
  assert.match(config, /allow = \["public"\][\s\S]*tlsOptions = \(trustBrowserCas = true\)/);
  assert.equal((config.match(/name = "PUBLIC_NETWORK", service = "internet"/g) ?? []).length, 2);
  assert.doesNotMatch(config, /outbound-gateway|gateway\/outbound/);

  const owners = [
    "packages/runtime/src/loader/host.ts",
    "packages/runtime/src/services/transport.ts",
    "packages/runtime/src/durable-objects/host.ts",
    "packages/runtime/src/workflows/host.ts",
  ].map(source).join("\n");
  assert.doesNotMatch(owners, /OutboundGateway|ctx\.exports\.Outbound/);
  assert.equal((owners.match(/globalOutbound: tenantGlobalOutbound\(/g) ?? []).length, 5);
  assert.match(source("packages/runtime/src/loader/host.ts"), /globalOutbound: null/);
  assert.doesNotMatch(source("packages/runtime/src/loader/bindings.ts"), /PUBLIC_NETWORK/);
  assert.equal(existsSync(resolve(root, "packages/runtime/src/gateway/outbound.ts")), false);
});
