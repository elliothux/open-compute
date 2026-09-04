import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import {
  isEnumDeclaration,
  isIdentifier,
  isModuleDeclaration,
  isVariableStatement,
  type Node,
  type Statement,
} from "typescript/unstable/ast";
import { canonicalize, fingerprintDeclarationSourceTwice, parseSourceFile } from "./types-ast.ts";
import {
  NON_TARGET_PUBLIC_PRODUCTS,
  PARTIAL_TARGET_SYMBOLS,
  PLATFORM_PRODUCTS,
  PUBLIC_PRODUCTS,
  TARGET_PRODUCT_DEVIATIONS,
  classifySymbol,
} from "./inventory-classification.ts";
import {
  TYPE_ONLY_TARGET_SYMBOLS,
  buildDeclarationIndex,
  collapse,
  collectStatements,
  declarationName,
  emptyCoverage,
  expandTargetDeclaration,
  exportAliases,
  namedDeclarations,
  qualify,
  type InventoryCoverage,
  type PendingMember,
} from "./inventory-expand.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const INVENTORY_PATH = join(ROOT, "share/cloudflare-capabilities.json");

export interface InventorySource {
  workers_types_version: string;
  git_head: string;
  package_sha256: string;
  index_sha256: string;
  ast_sha256: string;
}

export interface InventoryMember {
  id: string;
  product: string;
  symbol: string;
  member: string;
  kind: string;
  overload: number;
  readonly: boolean;
  optional: boolean;
  static: boolean;
  signature: string;
  signature_sha256: string;
  status: "supported" | "supported_with_deviation" | "blocked";
  compile_cases: string[];
  runtime_cases: string[];
  deviations: string[];
}

export interface InventoryProduct {
  status: "supported" | "supported_with_deviation" | "unsupported" | "blocked";
  kind: "target" | "platform" | "non_target";
  capability_version?: number;
  members: InventoryMember[];
  deviations: string[];
}

export interface CapabilityInventory {
  schema_version: 1;
  source: InventorySource;
  managementApi: Record<string, unknown>;
  workersObservability: Record<string, unknown>;
  wrangler: Record<string, unknown>;
  products: Record<string, InventoryProduct>;
}

export interface EvidenceRecord {
  id: string;
  status: InventoryMember["status"];
  compile_cases: string[];
  runtime_cases: string[];
  deviations: string[];
}

export interface InventoryReport {
  inventory: CapabilityInventory;
  encoded: string;
  coverage: InventoryCoverage;
}

function sha256(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function finalizeMembers(pending: PendingMember[], evidence: Map<string, EvidenceRecord>): InventoryMember[] {
  const overloads = new Map<string, number>();
  const members: InventoryMember[] = [];
  for (const item of pending) {
    const key = `${item.product}\0${item.symbol}\0${item.member}\0${item.kind}`;
    const overload = overloads.get(key) ?? 0;
    overloads.set(key, overload + 1);
    const id = `${item.product}::${item.symbol}::${item.member}:${item.kind}#${overload}`;
    const mapped = evidence.get(id);
    const status = mapped?.status ?? "blocked";
    const compile_cases = mapped?.compile_cases ?? [];
    const runtime_cases = mapped?.runtime_cases ?? [];
    const deviations = mapped?.deviations ?? [];
    if (status !== "blocked" && (compile_cases.length === 0 || runtime_cases.length === 0)) {
      throw new Error(`${id}: supported inventory records require compile and real-runtime cases`);
    }
    if (status === "blocked" && (compile_cases.length !== 0 || runtime_cases.length !== 0)) {
      throw new Error(`${id}: blocked records must not carry evidence cases`);
    }
    members.push({
      id,
      product: item.product,
      symbol: item.symbol,
      member: item.member,
      kind: item.kind,
      overload,
      readonly: item.readonly,
      optional: item.optional,
      static: item.static,
      signature: collapse(item.node.getText()),
      signature_sha256: sha256(`${JSON.stringify(canonicalize(item.node))}\n`),
      status,
      compile_cases,
      runtime_cases,
      deviations,
    });
  }
  members.sort((left, right) =>
    left.product.localeCompare(right.product)
    || left.symbol.localeCompare(right.symbol)
    || left.member.localeCompare(right.member)
    || left.kind.localeCompare(right.kind)
    || left.overload - right.overload
  );
  const ids = new Set<string>();
  for (const member of members) {
    if (ids.has(member.id)) throw new Error(`duplicate inventory member: ${member.id}`);
    ids.add(member.id);
  }
  return members;
}

export function parseMemberEvidence(raw: unknown): Map<string, EvidenceRecord> {
  if (raw === undefined) return new Map();
  if (!Array.isArray(raw)) throw new Error("catalog.memberEvidence must be an array");
  const evidence = new Map<string, EvidenceRecord>();
  for (const [index, row] of raw.entries()) {
    const item = record(row, `memberEvidence[${index}]`);
    const id = String(item.id ?? "");
    if (!id || evidence.has(id)) throw new Error(`duplicate or empty memberEvidence id at ${index}`);
    const status = item.status;
    if (status !== "supported" && status !== "supported_with_deviation" && status !== "blocked") {
      throw new Error(`${id}: invalid memberEvidence status`);
    }
    evidence.set(id, {
      id,
      status,
      compile_cases: Array.isArray(item.compileCases) ? item.compileCases.map(String) : [],
      runtime_cases: Array.isArray(item.runtimeCases) ? item.runtimeCases.map(String) : [],
      deviations: Array.isArray(item.deviations) ? item.deviations.map(String) : [],
    });
  }
  return evidence;
}

export function assertEvidenceBinding(memberIds: Iterable<string>, evidence: Map<string, EvidenceRecord>): void {
  const ids = new Set(memberIds);
  for (const id of evidence.keys()) {
    if (!ids.has(id)) throw new Error(`stale memberEvidence id: ${id}`);
  }
}

function loadEvidence(catalogPath: string): Map<string, EvidenceRecord> {
  const catalog = record(JSON.parse(readFileSync(catalogPath, "utf8")), "catalog");
  return parseMemberEvidence(catalog.memberEvidence);
}

function productStatus(kind: InventoryProduct["kind"], members: InventoryMember[], deviations: readonly string[]): InventoryProduct {
  if (kind === "non_target") {
    return { status: "unsupported", kind, members: [], deviations: [] };
  }
  if (kind === "platform") {
    const status = deviations.length ? "supported_with_deviation" : "supported";
    return { status, kind, capability_version: 1, members: [], deviations: [...deviations] };
  }
  const blocked = members.some(member => member.status === "blocked");
  if (blocked || members.length === 0) {
    return { status: "blocked", kind, members, deviations: [...deviations] };
  }
  const withDeviation = members.some(member => member.status === "supported_with_deviation") || deviations.length > 0;
  return {
    status: withDeviation ? "supported_with_deviation" : "supported",
    kind,
    capability_version: 1,
    members,
    deviations: [...deviations],
  };
}

export async function generateInventoryWithCoverage(): Promise<{ inventory: CapabilityInventory; coverage: InventoryCoverage }> {
  const lock = record(JSON.parse(readFileSync(join(ROOT, "packages/runtime/workerd.lock.json"), "utf8")), "workerd lock");
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  const workersTypesRoot = dirname(createRequire(join(ROOT, "packages/types/package.json"))
    .resolve("@cloudflare/workers-types/package.json"));
  const packageJson = record(JSON.parse(readFileSync(join(workersTypesRoot, "package.json"), "utf8")), "workers-types package");
  const sourceText = readFileSync(join(workersTypesRoot, "index.d.ts"));
  const sourceString = sourceText.toString("utf8");
  if (packageJson.version !== lockTypes.version) {
    throw new Error("installed workers-types version does not match the formal lock");
  }
  const fingerprint = await fingerprintDeclarationSourceTwice(sourceString);
  if (fingerprint.sha256 !== lockTypes.astSha256) {
    throw new Error("workers-types AST digest does not match the formal lock");
  }
  const sourceFile = await parseSourceFile(sourceString);
  const rows = collectStatements(sourceFile);
  const names = namedDeclarations(rows);
  if (names.length === 0) throw new Error("pinned workers-types produced no named declarations");
  const index = buildDeclarationIndex(rows);
  const pending: PendingMember[] = [];
  const coverage = emptyCoverage();
  coverage.named_declarations = names.length;
  const aliasesByPrefix = new Map<string, Map<string, string>>();
  for (const { prefix, statement } of rows) {
    if (isModuleDeclaration(statement) && statement.body !== undefined && "statements" in statement.body) {
      const name = declarationName(statement);
      if (name === undefined) continue;
      const nested = `${qualify(prefix, name)}.`;
      const statements = (statement.body as { statements: readonly Statement[] }).statements;
      aliasesByPrefix.set(nested, exportAliases(statements));
    }
  }
  aliasesByPrefix.set("", exportAliases(sourceFile.statements));
  for (const { prefix, statement } of rows) {
    if (isModuleDeclaration(statement) || isEnumDeclaration(statement)) continue;
    const name = isVariableStatement(statement) ? undefined : declarationName(statement);
    const symbolName = name === undefined ? undefined : qualify(prefix, name);
    let classification = symbolName === undefined
      ? classifySymbol(prefix === "" ? "(global)" : prefix.slice(0, -1))
      : classifySymbol(symbolName);
    const partial = symbolName === undefined ? undefined : PARTIAL_TARGET_SYMBOLS.get(symbolName);
    if (classification.class !== "target" && partial === undefined) continue;
    if (partial !== undefined) classification = { product: partial.product, class: "target" };
    if (isVariableStatement(statement)) {
      let anyTarget = false;
      for (const declaration of statement.declarationList.declarations) {
        if (!isIdentifier(declaration.name)) continue;
        if (classifySymbol(qualify(prefix, declaration.name.text)).class === "target") anyTarget = true;
      }
      if (!anyTarget) continue;
    }
    const expanded = expandTargetDeclaration(
      pending,
      classification.product,
      prefix,
      statement,
      aliasesByPrefix.get(prefix) ?? new Map(),
      index,
      partial?.members,
    );
    for (const declared of expanded.names) {
      coverage.target_declarations += 1;
      if (expanded.surface) {
        if (expanded.added === 0 && !TYPE_ONLY_TARGET_SYMBOLS.has(declared)) {
          throw new Error(`${declared}: target declaration has object/call/construct surface but produced no inventory members`);
        }
        if (expanded.added === 0) coverage.target_declarations_type_only += 1;
        else coverage.target_declarations_with_surface += 1;
      } else {
        coverage.target_declarations_type_only += 1;
      }
    }
  }
  const evidence = loadEvidence(join(ROOT, "test/conformance/catalog.json"));
  const members = finalizeMembers(pending, evidence);
  assertEvidenceBinding(members.map(member => member.id), evidence);
  const membersByProduct = new Map<string, InventoryMember[]>();
  for (const member of members) {
    const list = membersByProduct.get(member.product) ?? [];
    list.push(member);
    membersByProduct.set(member.product, list);
  }
  const products: Record<string, InventoryProduct> = {};
  for (const name of PUBLIC_PRODUCTS) {
    if (name in PLATFORM_PRODUCTS) {
      const platform = PLATFORM_PRODUCTS[name]!;
      products[name] = productStatus("platform", [], platform.deviations);
      continue;
    }
    if ((NON_TARGET_PUBLIC_PRODUCTS as readonly string[]).includes(name)) {
      products[name] = productStatus("non_target", [], []);
      membersByProduct.delete(name);
      continue;
    }
    products[name] = productStatus("target", membersByProduct.get(name) ?? [], TARGET_PRODUCT_DEVIATIONS[name] ?? []);
    membersByProduct.delete(name);
  }
  const leftover = [...membersByProduct.entries()].filter(([, list]) => list.length > 0);
  if (leftover.length) {
    const detail = leftover.map(([name, list]) => `${JSON.stringify(name)}:${list.slice(0, 5).map(member => member.id).join("|")}`).join("; ");
    throw new Error(`target members escaped public products: ${detail}`);
  }
  const p6 = record(JSON.parse(readFileSync(join(ROOT, "openapi/p6-capability.json"), "utf8")), "P6 capability");
  return {
    inventory: {
      schema_version: 1,
      source: {
        workers_types_version: String(lockTypes.version),
        git_head: String(lockTypes.gitHead ?? lock.revision),
        package_sha256: String(lockTypes.packageSha256),
        index_sha256: sha256(sourceText),
        ast_sha256: fingerprint.sha256,
      },
      managementApi: record(p6.managementApi, "P6 managementApi"),
      workersObservability: record(p6.workersObservability, "P6 workersObservability"),
      wrangler: record(p6.wrangler, "P6 wrangler"),
      products,
    },
    coverage,
  };
}

export async function generateInventory(): Promise<CapabilityInventory> {
  return (await generateInventoryWithCoverage()).inventory;
}

export function encodeInventory(inventory: CapabilityInventory): string {
  return `${JSON.stringify(inventory, null, 2)}\n`;
}

export async function generateInventoryTwice(): Promise<InventoryReport> {
  const first = await generateInventoryWithCoverage();
  const second = encodeInventory(await generateInventory());
  const encoded = encodeInventory(first.inventory);
  if (encoded !== second) throw new Error("inventory generation is not deterministic");
  return { inventory: first.inventory, encoded, coverage: first.coverage };
}

async function main(args: string[]): Promise<void> {
  const command = args[0];
  if (command !== "generate" && command !== "check") {
    throw new Error("usage: inventory.ts generate|check [outfile]");
  }
  const { encoded } = await generateInventoryTwice();
  if (command === "generate") {
    const outfile = args[1] === undefined ? INVENTORY_PATH : resolve(args[1]);
    writeFileSync(outfile, encoded);
    return;
  }
  const committed = readFileSync(INVENTORY_PATH);
  if (Buffer.from(encoded).equals(committed) === false && encoded !== committed.toString("utf8")) {
    throw new Error("share/cloudflare-capabilities.json drifted from the generated inventory");
  }
  if (encoded !== committed.toString("utf8")) {
    throw new Error("share/cloudflare-capabilities.json drifted from the generated inventory");
  }
}

const entry = process.argv[1];
if (entry !== undefined && import.meta.url === pathToFileURL(resolve(entry)).href) {
  await main(process.argv.slice(2));
}
