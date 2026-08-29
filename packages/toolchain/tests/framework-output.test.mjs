import assert from "node:assert/strict";
import { mkdtemp, mkdir, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { importFrameworkOutput } from "../src/import/framework-output.ts";

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), "open-compute-framework-output-"));
  t.after(() => rm(root, { recursive: true, force: true }));
  await mkdir(join(root, ".wrangler", "deploy"), { recursive: true });
  await mkdir(join(root, "dist", "server", "chunks"), { recursive: true });
  await mkdir(join(root, "dist", "client", "assets"), { recursive: true });
  await writeFile(join(root, ".wrangler", "deploy", "config.json"), JSON.stringify({
    configPath: "../../dist/server/wrangler.json",
  }));
  await writeFile(join(root, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: "2026-08-22",
    compatibility_flags: ["nodejs_compat"],
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
    project: root, name: "framework", compatibilityDate: "2026-08-22",
    compatibilityFlags: ["nodejs_compat"], frameworkOutput: ".wrangler/deploy/config.json",
  };
}

test("imports generated server modules and client assets without rebundling or flattening", async t => {
  const output = await importFrameworkOutput(await fixture(t));
  assert.equal(output.worker.mainModule, "index.js");
  assert.deepEqual(output.worker.modules.map(module => module.name), ["chunks/page-abc.js", "index.js"]);
  assert.match(new TextDecoder().decode(output.worker.modules[1].bytes), /chunks\/page-abc\.js/);
  assert.equal(output.assets.directory, "dist/client");
  assert.equal(output.assets.binding, "ASSETS");
  assert.deepEqual(output.assets.runWorkerFirst, ["/api/*"]);
});

test("rejects escaped outputs, links, metadata drift, and unsupported generated bindings", async t => {
  const project = await fixture(t);
  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: "2026-08-21",
  }));
  await assert.rejects(importFrameworkOutput(project), /compatibility date/);

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: "2026-08-22",
    compatibility_flags: ["nodejs_compat"], services: [{ binding: "SELF", service: "x" }],
  }));
  await assert.rejects(importFrameworkOutput(project), /declares bindings/);

  await writeFile(join(project.project, "dist", "server", "wrangler.json"), JSON.stringify({
    name: "framework", main: "index.js", compatibility_date: "2026-08-22",
    compatibility_flags: ["nodejs_compat"],
  }));
  await symlink(join(project.project, "dist", "server", "index.js"), join(project.project, "dist", "server", "linked.js"));
  await assert.rejects(importFrameworkOutput(project), /symbolic link/);
});
