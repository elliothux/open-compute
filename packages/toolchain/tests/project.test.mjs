import assert from "node:assert/strict";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { canonicalServiceProps, loadProject } from "../src/project.ts";

async function fixture(t, value, name = "wrangler.jsonc") {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-wrangler-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  await mkdir(join(directory, "src"), { recursive: true });
  const filename = join(directory, name);
  await writeFile(filename, typeof value === "string" ? value : JSON.stringify(value));
  return { directory, filename };
}

test("uses pinned Wrangler parsing and projects standard bindings", async t => {
  const { filename } = await fixture(t, {
    name: "hello",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
    vars: { GREETING: "hello", NESTED: { enabled: true } },
    secrets: { required: ["TOKEN"] },
    kv_namespaces: [{ binding: "KV", id: "kv-id" }],
    r2_buckets: [{ binding: "BUCKET", bucket_name: "files" }],
    d1_databases: [{ binding: "DB", database_name: "db", database_id: "d1-id" }],
    durable_objects: { bindings: [{ name: "OBJECTS", class_name: "PortableObject" }] },
    queues: { producers: [{ binding: "EVENTS", queue: "events" }] },
    workflows: [{ binding: "FLOW", name: "flow", class_name: "PortableWorkflow", schedules: ["0 * * * *"] }],
    vectorize: [{ binding: "VECTOR", index_name: "vectors" }],
    ai_search_namespaces: [{ binding: "SEARCH_NS", namespace: "team" }],
    ai_search: [{ binding: "SEARCH", instance_name: "docs" }],
    services: [{
      binding: "CATALOG",
      service: "catalog",
      entrypoint: "CatalogApi",
      props: { z: [1, { ordinary: "JSON data" }], constructor: { enabled: true } },
    }],
    images: { binding: "IMAGES" },
    ai: { binding: "AI" },
    version_metadata: { binding: "VERSION" },
    assets: { directory: "public", binding: "ASSETS", run_worker_first: ["/api/*"] },
  });
  const project = await loadProject(filename);
  assert.equal(project.configPath, "wrangler.jsonc");
  assert.equal(project.main, "src/index.ts");
  assert.deepEqual(project.secrets, ["TOKEN"]);
  assert.equal(project.bindings.DB.id, "d1-id");
  assert.equal(project.bindings.OBJECTS.className, "PortableObject");
  assert.deepEqual(project.services.CATALOG, {
    service: "catalog",
    entrypoint: "CatalogApi",
    props: { constructor: { enabled: true }, z: [1, { ordinary: "JSON data" }] },
  });
  assert.deepEqual(Object.keys(project.services.CATALOG.props), ["constructor", "z"]);
  assert.deepEqual(project.runtimeFeatures.images, { binding: "IMAGES" });
  assert.equal(project.assets.binding, "ASSETS");
});

test("canonicalizes arbitrary Service props and enforces JSON depth and size", () => {
  const props = canonicalServiceProps(JSON.parse('{"z":1,"__proto__":{"ok":true},"a":[null,false]}'));
  assert.deepEqual(Object.keys(props), ["__proto__", "a", "z"]);
  assert.equal(Object.hasOwn(props, "__proto__"), true);
  assert.throws(() => canonicalServiceProps({ value: Number.NaN }), /only JSON values/);
  assert.throws(() => canonicalServiceProps({ value: new Date(0) }), /only JSON values/);
  assert.throws(() => canonicalServiceProps({ value: "x".repeat(64 * 1024) }), /supported size/);
  let nested = true;
  for (let index = 0; index < 33; index += 1) nested = [nested];
  assert.throws(() => canonicalServiceProps({ nested }), /supported depth/);
});

test("loads the exact requested config when a directory contains multiple Wrangler configs", async t => {
  const { directory, filename } = await fixture(t, {
    name: "explicit",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
  }, "selected.jsonc");
  await writeFile(join(directory, "wrangler.jsonc"), JSON.stringify({
    name: "automatic",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
  }));
  await mkdir(join(directory, ".wrangler", "deploy"), { recursive: true });
  await writeFile(join(directory, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../generated.json",
    auxiliaryWorkers: [],
  }));
  await writeFile(join(directory, "generated.json"), JSON.stringify({
    name: "redirected-automatic",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
  }));
  const project = await loadProject(filename);
  assert.equal(project.name, "explicit");
  assert.equal(project.configPath, "selected.jsonc");
});

test("legacy project fields cannot drive the normalized Wrangler projection", async t => {
  const { filename } = await fixture(t, JSON.stringify({
    name: "hello",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
    frameworkOutput: ".wrangler/deploy/config.json",
  }));
  const project = await loadProject(filename);
  assert.equal(project.frameworkOutput, undefined);
  assert.equal(project.main, "src/index.ts");
});

test("rejects unsupported standard Wrangler bindings after normalization", async t => {
  const { filename } = await fixture(t, {
    name: "unsupported",
    main: "src/index.ts",
    compatibility_date: "2026-08-30",
    analytics_engine_datasets: [{ binding: "ANALYTICS", dataset: "events" }],
  });
  await assert.rejects(loadProject(filename), {
    message: "Wrangler config declares unsupported analytics_engine_datasets",
  });
});

test("consumes the standard generated deployment redirect", async t => {
  const { directory } = await fixture(t, {}, "placeholder");
  await writeFile(join(directory, "wrangler.jsonc"), JSON.stringify({
    name: "user-framework", main: "src/index.ts", compatibility_date: "2026-08-30",
  }));
  await mkdir(join(directory, ".wrangler", "deploy"), { recursive: true });
  await mkdir(join(directory, "dist", "server"), { recursive: true });
  await writeFile(join(directory, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json",
    auxiliaryWorkers: [],
  }));
  await writeFile(join(directory, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework",
    main: "index.js",
    no_bundle: true,
    compatibility_date: "2026-08-30",
  }));
  const project = await loadProject(join(directory, "wrangler.jsonc"));
  assert.equal(project.frameworkOutput, ".wrangler/deploy/config.json");
  assert.equal(project.main, undefined);
});
