import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { validateCaseEvidence } from "./case-evidence.ts";

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "open-compute-evidence-"));
  const directory = join(root, "test/conformance/fixtures/kv");
  mkdirSync(directory, { recursive: true });
  writeFileSync(join(root, "test/conformance/fixtures/tsconfig.json"), JSON.stringify({ include: ["**/*.ts"] }));
  writeFileSync(join(directory, "surface.ts"), "export {};\n");
  const runtime = "p0-4::p0_4_real_kv_matrix";
  const compile = "ts::test/conformance/fixtures/kv/surface.ts";
  const catalog = {
    contracts: [{ positiveCases: [runtime], negativeCases: [runtime] }],
    memberEvidence: [{ compileCases: [compile], runtimeCases: [runtime] }],
  };
  const registry = { schemaVersion: 1, cases: [runtime] };
  return { root, catalog, registry, runtime };
}

test("case evidence accepts exact registered runtime and contained compile fixtures", () => {
  const value = fixture();
  assert.doesNotThrow(() => validateCaseEvidence(value.root, value.catalog, value.registry));
});

test("case evidence rejects stale, duplicate, missing, and traversing identities", () => {
  const value = fixture();
  assert.throws(() => validateCaseEvidence(value.root, value.catalog, { schemaVersion: 1, cases: [] }), /unregistered/);
  assert.throws(() => validateCaseEvidence(value.root, value.catalog, {
    schemaVersion: 1, cases: [value.runtime, value.runtime],
  }), /duplicate evidence/);
  value.catalog.memberEvidence[0].compileCases = ["ts::test/conformance/fixtures/kv/missing.ts"];
  assert.throws(() => validateCaseEvidence(value.root, value.catalog, value.registry), /missing/);
  value.catalog.memberEvidence[0].compileCases = ["ts::test/conformance/fixtures/../outside.ts"];
  assert.throws(() => validateCaseEvidence(value.root, value.catalog, value.registry), /outside|escapes/);
});

test("case evidence rejects duplicate per-member references", () => {
  const value = fixture();
  value.catalog.memberEvidence[0].runtimeCases = [value.runtime, value.runtime];
  assert.throws(() => validateCaseEvidence(value.root, value.catalog, value.registry), /duplicate evidence/);
});
