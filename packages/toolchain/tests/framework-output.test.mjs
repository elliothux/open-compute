import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { importFrameworkOutput } from "../src/import/framework-output.ts";
import { loadFormalRuntimeLock } from "../src/runtime-lock.ts";

function generatedConfig(lock, overrides = {}) {
  return {
    name: "framework-cloudflare",
    main: "index.js",
    compatibility_date: lock.effectiveCompatibilityDate,
    rules: [{ type: "ESModule", globs: ["**/*.js"] }],
    no_bundle: true,
    ...overrides,
  };
}

async function fixture(t, wrangler = {}) {
  const lock = await loadFormalRuntimeLock();
  const root = await mkdtemp(join(tmpdir(), "open-compute-framework-output-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, ".wrangler", "deploy"), { recursive: true });
  await mkdir(join(root, "dist", "server", "chunks"), { recursive: true });
  await mkdir(join(root, "dist", "client", "assets"), { recursive: true });
  await writeFile(join(root, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json",
    auxiliaryWorkers: [],
  }));
  await writeFile(join(root, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    ...wrangler,
    assets: {
      directory: "../client", binding: "ASSETS", run_worker_first: ["/api/*"],
      html_handling: "auto-trailing-slash", not_found_handling: "none",
    },
  })));
  await writeFile(join(root, "dist", "server", "index.js"), "import('./chunks/page-abc.js'); export default {};");
  await writeFile(join(root, "dist", "server", "chunks", "page-abc.js"), "export const page = 1;");
  await writeFile(join(root, "dist", "server", "unused.js"), "export const unused = true;");
  await writeFile(join(root, "dist", "server", "qualification.txt"), "qualification\n");
  await writeFile(join(root, "dist", "server", "metadata.json"), "{\"not\":\"a module\"}\n");
  await writeFile(join(root, "dist", "client", "index.html"), "<main>app</main>");
  await writeFile(join(root, "dist", "client", "assets", "app-abc.js"), "globalThis.client = true;");
  return {
    project: root, configPath: ".wrangler/deploy/config.json", name: "framework-local", vars: {}, secrets: [], bindings: {}, services: {},
    frameworkOutput: ".wrangler/deploy/config.json",
    runtimeFeatures: {
      cache: { enabled: false, crossVersionCache: false, entrypoints: {} },
    },
  };
}

function withoutSelectors(value) {
  assert.equal(Object.hasOwn(value, "compatibilityDate"), false);
  assert.equal(Object.hasOwn(value, "compatibilityFlags"), false);
  assert.equal(Object.hasOwn(value, "compatibility_date"), false);
  assert.equal(Object.hasOwn(value, "compatibility_flags"), false);
}

test("imports generated server modules and client assets without rebundling or flattening", async t => {
  const output = await importFrameworkOutput(await fixture(t));
  assert.equal(output.worker.mainModule, "index.js");
  assert.deepEqual(output.worker.modules.map(module => module.name), [
    "chunks/page-abc.js", "index.js", "qualification.txt", "unused.js",
  ]);
  assert.match(new TextDecoder().decode(output.worker.modules[1].bytes), /chunks\/page-abc\.js/);
  assert.equal(output.assets.directory, "dist/client");
  assert.equal(output.assets.binding, "ASSETS");
  assert.deepEqual(output.assets.runWorkerFirst, ["/api/*"]);
  withoutSelectors(output);
});

test("accepts empty and redundant current-default flags without persisting selectors", async t => {
  for (const flags of [undefined, [], ["nodejs_compat"], ["rpc", "enable_ctx_exports", "nodejs_compat_v2", "nodejs_compat"]]) {
    const project = await fixture(t, flags === undefined ? {} : { compatibility_flags: flags });
    const output = await importFrameworkOutput(project);
    withoutSelectors(output);
    withoutSelectors(project);
  }
});

test("rejects missing, older, newer, duplicate, opt-out, experimental, and unknown flags", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  const wrangler = join(project.project, "dist", "server", "wrangler.json");
  const write = (body) => writeFile(wrangler, JSON.stringify(generatedConfig(lock, body)));

  await write({ compatibility_date: "2026-08-21" });
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);
  await write({ compatibility_date: "2026-08-31" });
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);
  await write({ compatibility_date: undefined });
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);
  await write({
    compatibility_date: lock.effectiveCompatibilityDate,
    compatibility_flags: ["nodejs_compat", "nodejs_compat"],
  });
  await assert.rejects(importFrameworkOutput(project), /duplicated/);
  await write({
    compatibility_date: lock.effectiveCompatibilityDate,
    compatibility_flags: ["no_nodejs_compat"],
  });
  await assert.rejects(importFrameworkOutput(project), /pinned baseline/);
  await write({
    compatibility_date: lock.effectiveCompatibilityDate,
    compatibility_flags: ["experimental"],
  });
  await assert.rejects(importFrameworkOutput(project), /pinned baseline/);
  await write({
    compatibility_date: lock.effectiveCompatibilityDate,
    compatibility_flags: ["unknown_flag"],
  });
  await assert.rejects(importFrameworkOutput(project), /pinned baseline/);
});

test("reconciles provider identities while preserving local services and binding resource IDs", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  project.services = { SELF: { service: "local-companion", entrypoint: "Api" } };
  project.bindings = { KV: { type: "kv_namespace", id: "local-kv" } };
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    services: [{ binding: "SELF", service: "cloudflare-companion", entrypoint: "Api" }],
    kv_namespaces: [{ binding: "KV", id: "cloudflare-kv" }],
  })));
  const imported = await importFrameworkOutput(project);
  assert.deepEqual(imported.services, { SELF: { service: "local-companion", entrypoint: "Api" } });
  withoutSelectors(imported);
});

test("imports Wrangler Vectorize and AI Search declarations while retaining local resource IDs", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  project.bindings = {
    VECTOR: { type: "vectorize_index", id: "local-vector" },
    SEARCH_NS: { type: "ai_search_namespace", id: "local-search-namespace" },
    SEARCH: { type: "ai_search_instance", id: "local-search-instance" },
  };
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    vectorize: [{ binding: "VECTOR", index_name: "provider-vector" }],
    ai_search_namespaces: [{ binding: "SEARCH_NS", namespace: "provider-namespace" }],
    ai_search: [{ binding: "SEARCH", instance_name: "provider-instance" }],
  })));
  await importFrameworkOutput(project);
});

test("rejects binding-shape drift, unsupported generated capabilities, auxiliary Workers, and links", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    kv_namespaces: [{ binding: "KV", id: "x" }],
  })));
  await assert.rejects(importFrameworkOutput(project), /bindings differ/);

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    ai: { binding: "AI" },
  })));
  const withAi = await importFrameworkOutput(project);
  assert.deepEqual(withAi.runtimeFeatures.ai, { binding: "AI" });

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock, {
    browser: { binding: "BROWSER" },
  })));
  await assert.rejects(importFrameworkOutput(project), /unsupported browser/);

  await writeFile(join(project.project, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json",
    auxiliaryWorkers: [{ configPath: "worker.json" }],
  }));
  await assert.rejects(importFrameworkOutput(project), /auxiliary Workers/);

  await writeFile(join(project.project, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json", auxiliaryWorkers: [],
  }));
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify(generatedConfig(lock)));
  await symlink(join(project.project, "dist", "server", "index.js"), join(project.project, "dist", "server", "linked.js"));
  await assert.rejects(importFrameworkOutput(project), /symbolic link/);
});

test("requires generated class-bound bindings to match local class names", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  project.bindings = {
    OBJECTS: { type: "do_namespace", id: "local-do", className: "PortableObject" },
    FLOW: { type: "workflow", id: "local-flow", className: "PortableWorkflow" },
  };
  const wrangler = join(project.project, "dist", "server", "wrangler.json");
  await writeFile(wrangler, JSON.stringify(generatedConfig(lock, {
    durable_objects: { bindings: [{ name: "OBJECTS", class_name: "PortableObject" }] },
    workflows: [{ binding: "FLOW", name: "provider-flow", class_name: "PortableWorkflow" }],
  })));
  await importFrameworkOutput(project);
  await writeFile(wrangler, JSON.stringify(generatedConfig(lock, {
    durable_objects: { bindings: [{ name: "OBJECTS", class_name: "DifferentObject" }] },
    workflows: [{ binding: "FLOW", name: "provider-flow", class_name: "PortableWorkflow" }],
  })));
  await assert.rejects(importFrameworkOutput(project), /bindings differ/);
});
