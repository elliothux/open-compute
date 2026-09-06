import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import test from "node:test";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

test("vinext current inputs are verified offline without renewing the historical P4 verdict", () => {
  const result = spawnSync("bun", ["test/conformance/applications/check-vinext.ts", "--list"], {
    cwd: root,
    encoding: "utf8",
    env: { PATH: process.env.PATH },
  });
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.application, "vinext");
  assert.equal(report.schemaVersion, 2);
  assert.equal(report.verdict, "inputs-verified");
  assert.equal(report.historicalVerdict, "go");
  assert.equal(report.rootLockSha256, createHash("sha256").update(readFileSync(resolve(root, "bun.lock"))).digest("hex"));
  assert.equal(report.historicalRootLockSha256, "c99e951a642e668e1826f56289b02c03abcb3d084675d3cd117205e25b49d3d0");
  assert.equal(report.mandatory, 20);
  assert.equal(report.optional, 0);
  assert.equal(report.excluded, 14);
  assert.equal(report.cases.length, 34);
  assert.equal(new Set(report.cases).size, report.cases.length);
});

// Copy tracked fixture inputs into an isolated repository so failure tests never
// mutate the real lock, dependencies, or retained qualification evidence.
test("vinext offline checks reject changed frozen inputs and installed packages", async (t) => {
  const parent = resolve(root, ".temp/vinext-lock-fix");
  mkdirSync(parent, { recursive: true });
  const sandbox = mkdtempSync(resolve(parent, "regression-"));
  t.after(() => rmSync(sandbox, { recursive: true, force: true }));
  const files = execFileSync("git", ["ls-files", "-z", "--", "test/applications/vinext"], { cwd: root })
    .toString().split("\0").filter(Boolean);
  files.push("bun.lock", "test/conformance/catalog.json",
    "test/conformance/applications/check-vinext.ts", "test/conformance/applications/vinext.json",
    "test/conformance/applications/vinext-cases.json");
  for (const name of files) {
    mkdirSync(dirname(resolve(sandbox, name)), { recursive: true });
    copyFileSync(resolve(root, name), resolve(sandbox, name));
  }
  writeFileSync(resolve(sandbox, ".gitignore"), "node_modules\n");
  execFileSync("git", ["init", "--quiet"], { cwd: sandbox });
  const dependencies = resolve(sandbox, "test/applications/vinext/node_modules");
  symlinkSync(resolve(root, "test/applications/vinext/node_modules"), dependencies, "dir");
  const run = () => spawnSync("bun", ["test/conformance/applications/check-vinext.ts", "--list"], {
    cwd: sandbox, encoding: "utf8", env: { PATH: process.env.PATH },
  });
  const initial = run();
  assert.equal(initial.status, 0, initial.stderr);
  for (const [name, error] of [
    ["bun.lock", "root lock digest drift"],
    ["test/applications/vinext/app/page.tsx", "fixture tree digest drift"],
    ["test/conformance/applications/vinext-cases.json", "case matrix digest drift"],
  ]) {
    await t.test(error, () => {
      const path = resolve(sandbox, name);
      const bytes = readFileSync(path);
      try {
        writeFileSync(path, Buffer.concat([bytes, Buffer.from("\n")]));
        const result = run();
        assert.notEqual(result.status, 0);
        assert.ok(result.stderr.includes(error), result.stderr);
      } finally {
        writeFileSync(path, bytes);
      }
    });
  }
  await t.test("installed package drift", () => {
    unlinkSync(dependencies);
    const directory = resolve(dependencies, "@cloudflare/vite-plugin");
    mkdirSync(directory, { recursive: true });
    writeFileSync(resolve(directory, "package.json"), JSON.stringify({ version: "0.0.0" }));
    const result = run();
    assert.notEqual(result.status, 0);
    assert.ok(result.stderr.includes("installed fixture package identity drift: @cloudflare/vite-plugin"), result.stderr);
  });
});
