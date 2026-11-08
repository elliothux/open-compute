import assert from "node:assert/strict";
import { realpathSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";

// Resolve the config implementation consumed by the repository-pinned cf CLI.
const cfRoot = realpathSync(new URL("../node_modules/cf", import.meta.url));
const configRoot = resolve(cfRoot, "../@cloudflare/config");
const configPackage = JSON.parse(
  await readFile(resolve(configRoot, "package.json"), "utf8"),
);
assert.equal(configPackage.version, "0.28.0");
const { bindings, convertToWranglerConfig, resolveAndParseConfig } =
  await import(pathToFileURL(resolve(configRoot, "dist/index.mjs")).href);

async function convert(bindingWorker) {
  const parsed = await resolveAndParseConfig(
    {
      worker: {
        name: "binding-owner",
        entrypoint: "./index.ts",
        compatibilityDate: "2026-09-08",
        env: {
          OBJECTS: bindings.durableObject({
            worker: bindingWorker,
            exportName: "ContainerObject",
          }),
        },
      },
    },
    { isPreview: false, mode: "production" },
  );
  assert.equal(parsed.success, true, parsed.error?.message);
  const result = convertToWranglerConfig(parsed.data);
  assert.equal(parsed.data.worker.env.OBJECTS.worker, bindingWorker);
  return result.durable_objects.bindings;
}

test("cf config omits script_name for a binding to the configured Worker", async () => {
  assert.deepEqual(await convert("binding-owner"), [
    { name: "OBJECTS", class_name: "ContainerObject" },
  ]);
});

test("cf config preserves script_name for an external Worker binding", async () => {
  assert.deepEqual(await convert("external-worker"), [
    {
      name: "OBJECTS",
      class_name: "ContainerObject",
      script_name: "external-worker",
    },
  ]);
});
