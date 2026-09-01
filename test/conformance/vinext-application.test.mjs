import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

test("vinext P4 Cloudflare-aligned baseline and finite case inventory are frozen offline", () => {
  const result = spawnSync("bun", ["test/conformance/applications/check-vinext.ts", "--list"], {
    cwd: root,
    encoding: "utf8",
    env: { PATH: process.env.PATH },
  });
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(result.stdout);
  assert.equal(report.application, "vinext");
  assert.equal(report.verdict, "go");
  assert.equal(report.mandatory, 20);
  assert.equal(report.optional, 0);
  assert.equal(report.excluded, 14);
  assert.equal(report.cases.length, 34);
  assert.equal(new Set(report.cases).size, report.cases.length);
});
