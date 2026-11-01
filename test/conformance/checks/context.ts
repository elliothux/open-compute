import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { JsonRecord } from "../adapters/types.ts";

export const ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../..",
);
export function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as JsonRecord;
}

export function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
}

export function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`${label} must be a string`);
  return value;
}

export function strings(value: unknown, label: string): string[] {
  return array(value, label).map((item, index) =>
    string(item, `${label}[${index}]`),
  );
}

export function json(path: string): unknown {
  return JSON.parse(readFileSync(join(ROOT, path), "utf8"));
}

export function sha256(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function digest(path: string): string {
  return sha256(readFileSync(join(ROOT, path)));
}

export function sourceIdentityExcluded(name: string): boolean {
  return (
    name.split("/").includes("__pycache__") ||
    ["AGENTS.md", "CLAUDE.md"].includes(basename(name)) ||
    name.startsWith(".agents/") ||
    name.startsWith(".claude/") ||
    name === "test/conformance/baseline.json" ||
    (name.startsWith("docs/") && !name.startsWith("docs/references/runbooks/"))
  );
}

export function sourceIdentity(): string {
  const names = execFileSync("git", ["ls-files", "-z", "--cached"], {
    cwd: ROOT,
  })
    .subarray(0, -1)
    .toString("utf8")
    .split("\0")
    .filter(Boolean)
    .sort();
  const regularNames = names.filter((name) => {
    if (sourceIdentityExcluded(name)) return false;
    try {
      return lstatSync(join(ROOT, name)).isFile();
    } catch {
      return false;
    }
  });
  const objectIds = execFileSync("git", ["hash-object", "--stdin-paths"], {
    cwd: ROOT,
    input: `${regularNames.join("\n")}\n`,
  })
    .toString("utf8")
    .trim()
    .split("\n");
  if (objectIds.length !== regularNames.length) {
    throw new Error("source identity object count differs from file count");
  }
  const objects = new Map(
    regularNames.map((name, index) => [name, objectIds[index]!]),
  );
  const output = createHash("sha256");
  for (const name of regularNames) {
    output.update(name);
    output.update("\0");
    output.update(objects.get(name)!);
  }
  return output.digest("hex");
}

export function baseline(): JsonRecord {
  return record(json("test/conformance/baseline.json"), "baseline");
}
export function catalog(): JsonRecord {
  return record(json("test/conformance/catalog.json"), "catalog");
}
export function inventory(): JsonRecord {
  return record(json("share/cloudflare-capabilities.json"), "inventory");
}
export function capabilities(): JsonRecord {
  return record(inventory().products, "inventory.products");
}

export function inventoryMembers(): JsonRecord[] {
  const members: JsonRecord[] = [];
  for (const [product, raw] of Object.entries(capabilities())) {
    const capability = record(raw, `capability ${product}`);
    for (const [index, item] of array(
      capability.members ?? [],
      `${product}.members`,
    ).entries()) {
      members.push(record(item, `${product}.members[${index}]`));
    }
  }
  return members;
}
export function contracts(): JsonRecord[] {
  return array(catalog().contracts, "catalog.contracts").map((item, index) =>
    record(item, `contract ${index}`),
  );
}

export function productNames(contract: JsonRecord): string[] {
  return [
    string(contract.product, "contract.product"),
    ...strings(contract.additionalProducts ?? [], "additionalProducts"),
  ];
}
