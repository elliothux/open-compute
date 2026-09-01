import { lstatSync, readFileSync } from "node:fs";
import { isAbsolute, relative, resolve, sep } from "node:path";

type JsonRecord = Record<string, unknown>;

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as JsonRecord;
}

function strings(value: unknown, label: string): string[] {
  if (!Array.isArray(value) || value.some(item => typeof item !== "string" || item.length === 0)) {
    throw new Error(`${label} must be a string array`);
  }
  if (new Set(value).size !== value.length) throw new Error(`${label} contains duplicate evidence`);
  return value as string[];
}

function contained(root: string, path: string): boolean {
  const child = relative(root, path);
  return child !== "" && child !== ".." && !child.startsWith(`..${sep}`) && !isAbsolute(child);
}

function compileFixture(root: string, id: string): void {
  if (!id.startsWith("ts::")) throw new Error(`invalid compile case identity: ${id}`);
  const source = id.slice(4);
  if (!source.startsWith("test/conformance/fixtures/") || !source.endsWith(".ts")) {
    throw new Error(`compile case is outside the fixture inventory: ${id}`);
  }
  const fixtures = resolve(root, "test/conformance/fixtures");
  const path = resolve(root, source);
  if (!contained(fixtures, path)) throw new Error(`compile case escapes fixture root: ${id}`);
  let regular = false;
  try { regular = lstatSync(path).isFile(); } catch { regular = false; }
  if (!regular) throw new Error(`compile case fixture is missing: ${id}`);
  const config = record(JSON.parse(readFileSync(resolve(fixtures, "tsconfig.json"), "utf8")), "fixture tsconfig");
  const include = strings(config.include, "fixture tsconfig.include");
  if (!include.includes("**/*.ts")) throw new Error("fixture tsconfig does not include every TypeScript fixture");
}

/** Validate exact compile/runtime evidence against the two authoritative registries. */
export function validateCaseEvidence(root: string, rawCatalog: unknown, rawRegistry: unknown): void {
  const catalog = record(rawCatalog, "catalog");
  const registry = record(rawRegistry, "Gate registry");
  if (registry.schemaVersion !== 1) throw new Error("unsupported Gate registry schema");
  const registered = strings(registry.cases, "Gate registry.cases");
  const registeredSet = new Set(registered);
  const runtime = new Set<string>();
  const compile = new Set<string>();

  if (!Array.isArray(catalog.contracts)) throw new Error("catalog.contracts must be an array");
  for (const [index, raw] of catalog.contracts.entries()) {
    const contract = record(raw, `contract[${index}]`);
    for (const field of ["positiveCases", "negativeCases"] as const) {
      for (const id of strings(contract[field], `contract[${index}].${field}`)) runtime.add(id);
    }
  }

  if (!Array.isArray(catalog.memberEvidence)) throw new Error("catalog.memberEvidence must be an array");
  for (const [index, raw] of catalog.memberEvidence.entries()) {
    const item = record(raw, `memberEvidence[${index}]`);
    for (const id of strings(item.compileCases ?? [], `memberEvidence[${index}].compileCases`)) {
      compileFixture(root, id);
      compile.add(id);
    }
    for (const id of strings(item.runtimeCases ?? [], `memberEvidence[${index}].runtimeCases`)) runtime.add(id);
  }

  const missing = [...runtime].filter(id => !registeredSet.has(id)).sort();
  if (missing.length) throw new Error(`catalog references unregistered Gate cases: ${missing.join(", ")}`);
  for (const id of runtime) {
    if (!/^[a-z0-9][a-z0-9-]*::\S+$/.test(id)) throw new Error(`invalid Gate case identity: ${id}`);
  }
  if (compile.size === 0 && catalog.memberEvidence.length !== 0) {
    throw new Error("member evidence registry contains no compile fixture");
  }
}
