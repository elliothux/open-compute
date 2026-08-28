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
  });
  const result = await loadProject(filename);
  assert.equal(result.main, config.main);
  assert.equal(result.tsconfig, "tsconfig.json");
  assert.equal(result.endpoint, "http://127.0.0.1:8787");
  assert.deepEqual(result.compatibilityFlags, []);
  assert.deepEqual(result.vars, { GREETING: "你好 🌍", nested: [null, true, { number: 42 }] });
  assert.deepEqual(result.secrets, { TOKEN: { env: "ABSENT_PROJECT_SECRET" } });
  assert.equal(result.bindings.DB.permissions.write, false);
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
