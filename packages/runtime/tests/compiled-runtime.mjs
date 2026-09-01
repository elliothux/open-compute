import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { transform } from "rolldown/utils";

const moduleRoot = fileURLToPath(new URL("../../../.temp/runtime-test-modules/", import.meta.url));
mkdirSync(moduleRoot, { recursive: true });
const modules = mkdtempSync(join(moduleRoot, "run-"));
let moduleOrdinal = 0;

export const moduleUrl = source => {
  const path = join(modules, `module-${moduleOrdinal++}.mjs`);
  writeFileSync(path, source);
  return pathToFileURL(path).href;
};

export async function compileRuntime(name, imports = {}) {
  const source = await readFile(new URL(`../src/${name}`, import.meta.url), "utf8");
  const result = await transform(name, source, {
    target: "esnext",
    sourcemap: false,
    tsconfig: fileURLToPath(new URL("../tsconfig.json", import.meta.url)),
  });
  assert.deepEqual(result.errors, [], name);
  assert.deepEqual(result.warnings, [], name);
  let code = result.code;
  for (const [specifier, replacement] of Object.entries(imports)) {
    assert.ok(code.includes(JSON.stringify(specifier)), `missing import ${specifier} in ${name}`);
    code = code.replaceAll(JSON.stringify(specifier), JSON.stringify(replacement));
  }
  return code;
}

export async function importRuntime(name, imports) {
  return import(moduleUrl(await compileRuntime(name, imports)));
}
