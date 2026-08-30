import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { loadProject } from "../src/project.ts";

const config = { name: "hello", main: "src/index.ts", compatibilityDate: "2026-08-22" };

async function fixture(t, value) {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-project-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const filename = join(directory, "open-compute.json");
  await writeFile(filename, typeof value === "string" ? value : JSON.stringify(value));
  return filename;
}

test("loads project-relative inputs and preserves JSON without reading secret values", async t => {
  const filename = await fixture(t, {
    ...config, vars: { GREETING: "你好 🌍", nested: [null, true, { number: 42 }] },
    secrets: { TOKEN: { env: "ABSENT_PROJECT_SECRET" } },
    bindings: { DB: { type: "d1_database", id: "resource-id", permissions: { read: true, write: false } } },
    services: [{ binding: "CATALOG", service: "catalog", entrypoint: "CatalogApi" }],
  });
  const result = await loadProject(filename);
  assert.equal(result.main, config.main);
  assert.equal(result.tsconfig, "tsconfig.json");
  assert.equal(result.endpoint, "http://127.0.0.1:8787");
  assert.deepEqual(result.compatibilityFlags, []);
  assert.deepEqual(result.vars, { GREETING: "你好 🌍", nested: [null, true, { number: 42 }] });
  assert.deepEqual(result.secrets, { TOKEN: { env: "ABSENT_PROJECT_SECRET" } });
  assert.equal(result.bindings.DB.permissions.write, false);
  assert.deepEqual(result.services, { CATALOG: { service: "catalog", entrypoint: "CatalogApi" } });
  assert.deepEqual(result.runtimeFeatures, {
    cache: { enabled: false, crossVersionCache: false, entrypoints: {} },
  });
});

test("parses cache, entrypoint, Images, and Version Metadata as one strict runtime contract", async t => {
  const result = await loadProject(await fixture(t, {
    ...config,
    cache: { enabled: true, cross_version_cache: true },
    exports: {
      default: { type: "worker", cache: { enabled: false } },
      Admin: { type: "worker", cache: { enabled: true, cross_version_cache: false } },
    },
    images: { binding: "IMAGES" },
    version_metadata: { binding: "VERSION", tag: "release-1" },
  }));
  assert.deepEqual(result.runtimeFeatures, {
    cache: {
      enabled: false,
      crossVersionCache: true,
      entrypoints: { Admin: { enabled: true, crossVersionCache: false } },
    },
    images: { binding: "IMAGES" },
    versionMetadata: { binding: "VERSION", tag: "release-1" },
  });
  for (const invalid of [
    { cache: { enabled: true, unknown: true } },
    { exports: { Bad: { type: "durable_object", cache: { enabled: true } } } },
    { images: { binding: "1BAD" } },
    { images: { binding: "SAME" }, version_metadata: { binding: "SAME" } },
    { version_metadata: { binding: "VERSION", tag: "" } },
  ]) {
    await assert.rejects(loadProject(await fixture(t, { ...config, ...invalid })));
  }
});

test("malformed config and plaintext secrets fail without echoing their contents", async t => {
  for (const extra of [
    { unknown: "sensitive-content" }, { name: "UPPERCASE" },
    { compatibilityDate: "tomorrow" }, { compatibilityFlags: null }, { compatibilityFlags: [1] },
    { vars: null }, { vars: [] }, { secrets: { TOKEN: "sensitive-content" } },
    { secrets: { TOKEN: { env: "VALID", value: "sensitive-content" } } },
    { bindings: { DB: { type: "unknown", id: "id" } } },
    { bindings: { DB: { type: "d1_database", id: "id", capabilityVersion: 3 } } },
    { bindings: { DB: { type: "d1_database", id: "id", permissions: { read: true } } } },
    { services: {} },
    { services: [{ binding: "1BAD", service: "catalog" }] },
    { services: [{ binding: "SELF", service: "UPPERCASE" }] },
    { services: [{ binding: "SELF", service: "hello", unknown: true }] },
  ]) {
    const filename = await fixture(t, { ...config, ...extra });
    await assert.rejects(loadProject(filename), error => {
      assert.doesNotMatch(error.message, /sensitive-content/);
      return true;
    });
  }
  await assert.rejects(loadProject(await fixture(t, "{sensitive-content}")), /valid JSON/);
  await assert.rejects(loadProject(await fixture(t, " ".repeat(64 * 1024 + 1))), /64 KiB/);
});

test("dictionary keys cannot modify prototypes", async t => {
  const filename = await fixture(t, `{"name":"hello","main":"index.ts","compatibilityDate":"2026-08-22",
    "vars":{"__proto__":{"polluted":true}},"secrets":{"__proto__":{"env":"PRIVATE_TOKEN"}}}`);
  const project = await loadProject(filename);
  assert.equal(Object.getPrototypeOf(project.vars), Object.prototype);
  assert.equal(Object.getPrototypeOf(project.secrets), Object.prototype);
  assert.deepEqual(Object.getOwnPropertyDescriptor(project.vars, "__proto__").value, { polluted: true });
  assert.equal({}.polluted, undefined);
});

test("loads Worker-plus-assets and assets-only project unions", async t => {
  const combined = await loadProject(await fixture(t, {
    ...config,
    assets: {
      directory: "dist", binding: "ASSETS", run_worker_first: ["/api/*"],
      not_found_handling: "single-page-application",
    },
  }));
  assert.equal(combined.assets.binding, "ASSETS");
  assert.equal(combined.assets.htmlHandling, "auto-trailing-slash");
  const staticOnly = await loadProject(await fixture(t, {
    name: "static", compatibilityDate: "2026-08-22", assets: { directory: "dist" },
  }));
  assert.equal(staticOnly.main, undefined);
  assert.equal(staticOnly.assets.runWorkerFirst, false);
  await assert.rejects(loadProject(await fixture(t, {
    ...config, vars: { ASSETS: true }, assets: { directory: "dist", binding: "ASSETS" },
  })), /conflicts/);
  await assert.rejects(loadProject(await fixture(t, {
    ...config, vars: { SELF: true }, services: [{ binding: "SELF", service: "hello" }],
  })), /conflicts/);
  await assert.rejects(loadProject(await fixture(t, {
    name: "static", compatibilityDate: "2026-08-22", vars: { MODE: "x" }, assets: { directory: "dist" },
  })), /assets-only/);
  await assert.rejects(loadProject(await fixture(t, {
    name: "static", compatibilityDate: "2026-08-22",
    services: [{ binding: "SELF", service: "static" }], assets: { directory: "dist" },
  })), /assets-only/);
  await assert.rejects(loadProject(await fixture(t, {
    ...config, assets: { directory: "dist", runWorkerFirst: true },
  })), /unknown assets field/);
});

test("loads a framework output union without retaining a second source entry", async t => {
  const project = await loadProject(await fixture(t, {
    name: "framework", compatibilityDate: "2026-08-22",
    frameworkOutput: ".wrangler/deploy/config.json",
  }));
  assert.equal(project.main, undefined);
  assert.equal(project.assets, undefined);
  assert.equal(project.frameworkOutput, ".wrangler/deploy/config.json");
  await assert.rejects(loadProject(await fixture(t, {
    ...config, frameworkOutput: ".wrangler/deploy/config.json",
  })), /cannot be combined/);
});
