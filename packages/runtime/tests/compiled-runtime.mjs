import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { transform } from "rolldown/utils";

export const moduleUrl = source => `data:text/javascript;base64,${Buffer.from(source).toString("base64")}`;

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
