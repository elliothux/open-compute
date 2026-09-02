import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { wranglerArgs, wranglerEntrypoint } from "../src/cli.ts";

test("oc delegates only deployment transport to Wrangler", () => {
  assert.deepEqual(wranglerArgs(["deploy", "--config", "wrangler.jsonc", "--env", "staging"]),
    ["deploy", "--config", "wrangler.jsonc", "--env", "staging"]);
  assert.deepEqual(wranglerArgs(["run", "--config", ".wrangler/deploy/config.json"]),
    ["deploy", "--config", ".wrangler/deploy/config.json"]);
  assert.throws(() => wranglerArgs(["build", "--outdir", "dist"]));
  assert.throws(() => wranglerArgs(["types", "worker-configuration.d.ts"]));
});

test("wrapper resolves the exact directly pinned Wrangler", async () => {
  const entrypoint = wranglerEntrypoint();
  assert.match(entrypoint, /wrangler@4\.127\.1/);
  assert.match(await readFile(entrypoint, "utf8"), /wrangler/);
});

test("local build source does not retain the removed private deployment package", async () => {
  const source = await readFile(new URL("../src/cli.ts", import.meta.url), "utf8");
  for (const removed of ["deploymentPackage", "contentKind", "bytesBase64", "manifest: assets", "routing: assets"]) {
    assert.equal(source.includes(removed), false, `removed private package token remains: ${removed}`);
  }
});
