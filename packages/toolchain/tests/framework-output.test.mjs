import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { importFrameworkOutput } from "../src/import/framework-output.ts";
import { loadFormalRuntimeLock } from "../src/runtime-lock.ts";

async function fixture(t, wrangler = {}) {
  const lock = await loadFormalRuntimeLock();
  const root = await mkdtemp(join(tmpdir(), "open-compute-framework-output-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, ".wrangler", "deploy"), { recursive: true });
  await mkdir(join(root, "dist", "server", "chunks"), { recursive: true });
  await mkdir(join(root, "dist", "client", "assets"), { recursive: true });
  await writeFile(join(root, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json",
  }));
  await writeFile(join(root, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: lock.effectiveCompatibilityDate,
    ...wrangler,
    assets: {
      directory: "../client", binding: "ASSETS", run_worker_first: ["/api/*"],
      html_handling: "auto-trailing-slash", not_found_handling: "none",
    },
  }));
  await writeFile(join(root, "dist", "server", "index.js"), "import('./chunks/page-abc.js'); export default {};");
  await writeFile(join(root, "dist", "server", "chunks", "page-abc.js"), "export const page = 1;");
  await writeFile(join(root, "dist", "client", "index.html"), "<main>app</main>");
  await writeFile(join(root, "dist", "client", "assets", "app-abc.js"), "globalThis.client = true;");
  return {
    project: root, name: "framework", services: {}, frameworkOutput: ".wrangler/deploy/config.json",
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
  assert.deepEqual(output.worker.modules.map(module => module.name), ["chunks/page-abc.js", "index.js"]);
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
  const write = (body) => writeFile(wrangler, JSON.stringify({ name: "framework", main: "index.js", ...body }));

  await write({ compatibility_date: "2026-08-21" });
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);
  await write({ compatibility_date: "2026-08-31" });
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);
  await write({});
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

test("imports services but rejects escaped outputs, links, metadata drift, and unsupported generated bindings", async t => {
  const lock = await loadFormalRuntimeLock();
  const project = await fixture(t);
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: lock.effectiveCompatibilityDate,
    services: [{ binding: "SELF", service: "x" }],
  }));
  const imported = await importFrameworkOutput(project);
  assert.deepEqual(imported.services, { SELF: { service: "x" } });
  withoutSelectors(imported);

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: lock.effectiveCompatibilityDate,
    kv_namespaces: [{ binding: "KV", id: "x" }],
  }));
  await assert.rejects(importFrameworkOutput(project), /declares bindings/);

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: lock.effectiveCompatibilityDate,
  }));
  await symlink(join(project.project, "dist", "server", "index.js"), join(project.project, "dist", "server", "linked.js"));
  await assert.rejects(importFrameworkOutput(project), /symbolic link/);
});
