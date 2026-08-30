import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { lstatSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadProject } from "../../packages/toolchain/src/project.ts";
import {
  cloudflareDeploymentUrl, cloudflareTransientFailure, cloudflareWorkerMissing,
  loadPortableFixtures, observationUrl, openComputeProject,
} from "./adapters.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const CASES = [
  "baseline-identity",
  "catalog-schema",
  "capability-catalog-bijection",
  "case-registry-mapping",
  "deviation-bijection",
  "compatibility-coverage",
  "public-types-surface",
  "unsupported-config-rejection",
  "portable-fixture-inventory",
  "cloudflare-runner-safety",
] as const;
type CaseId = typeof CASES[number];
type JsonRecord = Record<string, unknown>;

function record(value: unknown, label: string): JsonRecord {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as JsonRecord;
}

function array(value: unknown, label: string): unknown[] {
  if (!Array.isArray(value)) throw new Error(`${label} must be an array`);
  return value;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`${label} must be a string`);
  return value;
}

function strings(value: unknown, label: string): string[] {
  return array(value, label).map((item, index) => string(item, `${label}[${index}]`));
}

function json(path: string): unknown {
  return JSON.parse(readFileSync(join(ROOT, path), "utf8"));
}

function sha256(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function digest(path: string): string {
  return sha256(readFileSync(join(ROOT, path)));
}

function sourceIdentity(): string {
  const names = execFileSync("git", ["-c", "core.excludesFile=/dev/null", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], {
    cwd: ROOT,
  }).subarray(0, -1).toString("utf8").split("\0").filter(Boolean).sort();
  const output = createHash("sha256");
  for (const name of names) {
    if (name === "test/conformance/baseline.json"
        || (name.startsWith("docs/") && !name.startsWith("docs/references/"))) continue;
    output.update(name);
    output.update("\0");
    const path = join(ROOT, name);
    let regular = false;
    try { regular = lstatSync(path).isFile(); } catch { regular = false; }
    output.update(regular ? digest(name) : "deleted");
  }
  return output.digest("hex");
}

function baseline(): JsonRecord { return record(json("test/conformance/baseline.json"), "baseline"); }
function catalog(): JsonRecord { return record(json("test/conformance/catalog.json"), "catalog"); }
function capabilities(): JsonRecord { return record(json("share/cloudflare-capabilities.json"), "capabilities"); }
function contracts(): JsonRecord[] {
  return array(catalog().contracts, "catalog.contracts").map((item, index) => record(item, `contract ${index}`));
}

function productNames(contract: JsonRecord): string[] {
  return [string(contract.product, "contract.product"), ...strings(contract.additionalProducts ?? [], "additionalProducts")];
}

function baselineIdentity(): void {
  const value = baseline();
  if (value.schemaVersion !== 1) throw new Error("unsupported baseline schema");
  const hash = string(value.openComputeRevision, "openComputeRevision");
  const actualSource = sourceIdentity();
  if (!/^[0-9a-f]{64}$/.test(hash) || hash !== actualSource) {
    throw new Error(`open-compute source digest drift: expected=${hash}; actual=${actualSource}`);
  }
  if (string(value.workerdLockSha256, "workerdLockSha256") !== digest("packages/runtime/workerd.lock.json")) {
    throw new Error("workerd lock digest drift");
  }
  const lock = record(json("packages/runtime/workerd.lock.json"), "workerd lock");
  const workerd = record(value.workerd, "baseline.workerd");
  if (lock.release !== workerd.release || lock.expectedVersionOutput !== "workerd 2026-08-26") {
    throw new Error("workerd release identity drift");
  }
  const workersTypes = record(value.workersTypes, "workersTypes");
  if (workersTypes.version !== "5.20260826.1" || workersTypes.lockSha256 !== digest("bun.lock")) {
    throw new Error("workers-types lock identity drift");
  }
  const sdk = record(value.workersSdk, "workersSdk");
  if (!/^[0-9a-f]{40}$/.test(string(sdk.revision, "workersSdk.revision"))
      || !/^[0-9a-f]{64}$/.test(string(sdk.lockSha256, "workersSdk.lockSha256"))) {
    throw new Error("workers-sdk identity is not immutable");
  }
  const docs = record(value.cloudflareDocs, "cloudflareDocs");
  if (!/^[0-9a-f]{40}$/.test(string(docs.revision, "cloudflareDocs.revision"))
      || !/^[0-9a-f]{64}$/.test(string(docs.treeSha256, "cloudflareDocs.treeSha256"))) {
    throw new Error("Cloudflare docs identity is not immutable");
  }
}

function catalogSchema(): void {
  const value = catalog();
  if (value.schemaVersion !== 1) throw new Error("unsupported catalog schema");
  const ids = new Set<string>();
  for (const contract of contracts()) {
    const id = string(contract.id, "contract.id");
    if (!/^[a-z0-9]+(?:[.-][a-z0-9]+)*$/.test(id) || ids.has(id)) throw new Error(`duplicate or invalid contract id: ${id}`);
    ids.add(id);
    const status = string(contract.status, `${id}.status`);
    if (!["supported", "supported_with_deviation", "unsupported", "blocked"].includes(status)) {
      throw new Error(`${id}: invalid status`);
    }
    const methods = strings(contract.methods, `${id}.methods`);
    if (new Set(methods).size !== methods.length) throw new Error(`${id}: duplicate method`);
    const positive = strings(contract.positiveCases, `${id}.positiveCases`);
    const negative = strings(contract.negativeCases, `${id}.negativeCases`);
    if ((status === "supported" || status === "supported_with_deviation")
        && (!positive.length || !negative.length)) throw new Error(`${id}: supported contract lacks positive or negative evidence`);
    if (!array(contract.sources, `${id}.sources`).length) throw new Error(`${id}: source is missing`);
    for (const raw of array(contract.sources, `${id}.sources`)) {
      const source = record(raw, `${id}.source`);
      const revision = string(source.revision, `${id}.source.revision`);
      const sourcePath = string(source.path, `${id}.source.path`);
      if (!/^[0-9a-f]{40}$/.test(revision) || sourcePath.startsWith("/") || sourcePath.includes("..")
          || !/^[0-9a-f]{64}$/.test(string(source.sha256, `${id}.source.sha256`))) {
        throw new Error(`${id}: source identity is not immutable`);
      }
      if (source.kind === "cloudflare-doc") {
        const url = string(source.url, `${id}.source.url`);
        if (!url.includes(`/blob/${revision}/${sourcePath}`)) throw new Error(`${id}: Cloudflare source URL is not revision-pinned`);
      }
    }
  }
}

function capabilityCatalogBijection(): void {
  const advertised = capabilities();
  const mapped = new Map<string, JsonRecord>();
  for (const contract of contracts()) {
    for (const product of productNames(contract)) {
      if (mapped.has(product)) throw new Error(`product ${product} has more than one catalog owner`);
      mapped.set(product, contract);
    }
  }
  if (Object.keys(advertised).sort().join("\0") !== [...mapped.keys()].sort().join("\0")) {
    throw new Error("capability and catalog product inventories differ");
  }
  for (const [product, raw] of Object.entries(advertised)) {
    const capability = record(raw, `capability ${product}`);
    const contract = mapped.get(product);
    if (contract === undefined || capability.status !== contract.status) throw new Error(`${product}: status differs`);
    const capabilityMethods = strings(capability.methods ?? [], `${product}.methods`).sort();
    const contractMethods = product === contract.product ? strings(contract.methods, `${product}.contractMethods`).sort() : [];
    if (capabilityMethods.join("\0") !== contractMethods.join("\0")) throw new Error(`${product}: methods differ`);
    const capabilityDeviations = strings(capability.deviations ?? [], `${product}.deviations`).sort();
    const contractDeviations = product === contract.product ? strings(contract.deviations, `${product}.contractDeviations`).sort() : [];
    if (capabilityDeviations.join("\0") !== contractDeviations.join("\0")) throw new Error(`${product}: deviations differ`);
  }
}

function caseRegistryMapping(): void {
  for (const contract of contracts()) {
    for (const id of [...strings(contract.positiveCases, "positiveCases"), ...strings(contract.negativeCases, "negativeCases")]) {
      if (!/^[a-z0-9][a-z0-9-]*::[^\s]+$/.test(id)) throw new Error(`invalid Gate case identity: ${id}`);
    }
  }
}

function deviationBijection(): void {
  const registry = readFileSync(join(ROOT, "docs/references/p1-deviations.md"), "utf8");
  const documented = [...registry.matchAll(/`(OC-[A-Z0-9-]+)`:/g)].flatMap(match =>
    match[1] === undefined ? [] : [match[1]]);
  const advertised = new Set<string>();
  const mapped = new Set<string>();
  for (const raw of Object.values(capabilities())) {
    for (const id of strings(record(raw, "capability").deviations ?? [], "deviations")) advertised.add(id);
  }
  for (const contract of contracts()) for (const id of strings(contract.deviations, "contract.deviations")) mapped.add(id);
  const normalized = (items: Iterable<string>) => [...items].sort().join("\0");
  if (normalized(documented) !== normalized(advertised) || normalized(advertised) !== normalized(mapped)) {
    throw new Error("deviation registry, capabilities, and catalog differ");
  }
}

function compatibilityCoverage(): void {
  const base = baseline();
  const dates = new Set(strings(base.compatibilityDates, "compatibilityDates"));
  const baselineFlagSets = array(base.compatibilityFlags, "compatibilityFlags")
    .map((item, index) => strings(item, `compatibilityFlags[${index}]`));
  const flagSets = new Set(baselineFlagSets.map(flags => flags.join("\0")));
  const descriptor = readFileSync(join(ROOT, "crates/workers/src/descriptor.rs"), "utf8");
  const minimum = /COMPATIBILITY_DATE_MIN: &str = "([0-9-]+)"/.exec(descriptor)?.[1];
  const maximum = /COMPATIBILITY_DATE_MAX: &str = "([0-9-]+)"/.exec(descriptor)?.[1];
  if (minimum === undefined || maximum === undefined || !dates.has(minimum) || !dates.has(maximum)) {
    throw new Error("compatibility date boundary is not tested by the baseline");
  }
  for (const contract of contracts()) {
    const compatibility = record(contract.compatibility, `${contract.id}.compatibility`);
    if (compatibility.from !== minimum || compatibility.to !== maximum) throw new Error(`${contract.id}: date range differs`);
    for (const flags of array(compatibility.flags, `${contract.id}.flags`)) {
      if (!flagSets.has(strings(flags, `${contract.id}.flagSet`).join("\0"))) throw new Error(`${contract.id}: untested flag set`);
    }
  }
  const allowedBlock = /COMPATIBILITY_FLAGS_ALLOWED: &\[&str\] = &\[([\s\S]*?)\];/.exec(descriptor)?.[1];
  if (allowedBlock === undefined) throw new Error("compatibility flag allowlist is missing");
  const allowedFlags = [...allowedBlock.matchAll(/"([^"]+)"/g)].flatMap(match => match[1] === undefined ? [] : [match[1]]);
  const baselineFlags = [...new Set(baselineFlagSets.flat())];
  if (allowedFlags.sort().join("\0") !== baselineFlags.sort().join("\0")) {
    throw new Error("descriptor and baseline compatibility flag inventories differ");
  }
  for (const flag of allowedFlags) {
    if (!flagSets.has(flag)) throw new Error(`allowed flag lacks a dedicated baseline probe: ${flag}`);
  }
}

function publicTypesSurface(): void {
  const source = readFileSync(join(ROOT, "packages/types/index.d.ts"), "utf8");
  for (const contract of contracts()) {
    for (const name of strings(contract.typeInterfaces, `${contract.id}.typeInterfaces`)) {
      if (!new RegExp(`(?:interface|class)\\s+${name}\\b`).test(source)) throw new Error(`public type is missing: ${name}`);
    }
    for (const name of strings(contract.forbiddenTypes ?? [], `${contract.id}.forbiddenTypes`)) {
      if (new RegExp(`(?:interface|class|type)\\s+${name}\\b`).test(source)) throw new Error(`unsupported type is advertised: ${name}`);
    }
  }
  const example = readFileSync(join(ROOT, "examples/hello-worker/tsconfig.json"), "utf8");
  if (example.includes("@cloudflare/workers-types") || !example.includes("@open-compute/workers-types")) {
    throw new Error("example does not consume the authoritative supported type surface");
  }
}

async function unsupportedConfigRejection(): Promise<void> {
  const directory = mkdtempSync(join(tmpdir(), "open-compute-p3-contract-"));
  try {
    for (const type of ["analytics_engine", "ai", "browser", "vectorize", "hyperdrive", "mtls_certificate", "rate_limit", "worker_loader"]) {
      const path = join(directory, `${type}.json`);
      writeFileSync(path, JSON.stringify({
        main: "worker.ts", name: "unsupported-probe", tsconfig: "tsconfig.json",
        compatibilityDate: "2026-08-26", compatibilityFlags: [], vars: {}, secrets: {},
        bindings: { BAD: { type, id: "019c0000-0000-7000-8000-000000000001" } }, services: [],
      }));
      let rejected = false;
      try { await loadProject(path); } catch (error) {
        rejected = error instanceof Error && error.message === "unsupported Worker binding type";
      }
      if (!rejected) throw new Error(`unsupported binding was accepted: ${type}`);
    }
  } finally { rmSync(directory, { recursive: true }); }
}

function portableFixtureInventory(): void {
  const root = join(ROOT, "test/conformance/fixtures");
  const paths = execFileSync("find", [root, "-name", "contract.json", "-type", "f"], { encoding: "utf8" })
    .trim().split("\n").filter(Boolean).sort();
  if (!paths.length) throw new Error("portable fixture inventory is empty");
  const ids = new Set<string>();
  const catalogIds = new Set(contracts().map(contract => string(contract.id, "contract.id")));
  for (const path of paths) {
    const contract = record(JSON.parse(readFileSync(path, "utf8")), `fixture ${relative(ROOT, path)}`);
    const id = string(contract.id, "fixture.id");
    if (ids.has(id)) throw new Error(`duplicate fixture id: ${id}`);
    ids.add(id);
    const source = resolve(dirname(path), string(contract.source, `${id}.source`));
    if (!source.startsWith(`${dirname(path)}/`) || !lstatSync(source).isFile()) throw new Error(`${id}: invalid source`);
    if (readFileSync(source, "utf8").includes("OPEN_COMPUTE")) throw new Error(`${id}: target-specific Worker branch`);
    for (const mapped of strings(contract.contracts, `${id}.contracts`)) {
      if (!catalogIds.has(mapped)) throw new Error(`${id}: unknown contract mapping`);
    }
    if (!array(contract.observations, `${id}.observations`).length) throw new Error(`${id}: no observations`);
    const cleanup = record(contract.cleanup, `${id}.cleanup`);
    if (!array(cleanup.cloudflare, `${id}.cleanup.cloudflare`).length
        || !array(cleanup.openCompute, `${id}.cleanup.openCompute`).length) throw new Error(`${id}: incomplete cleanup ownership`);
  }
}

async function cloudflareRunnerSafety(): Promise<void> {
  const name = "oc-p34-test-1234";
  const url = cloudflareDeploymentUrl(`Uploaded fixture\nhttps://${name}.account.workers.dev`, name);
  if (url !== `https://${name}.account.workers.dev/`) throw new Error("Wrangler deployment URL parsing differs");
  if (!cloudflareWorkerMissing("Worker not found [code: 10007]")
      || !cloudflareWorkerMissing("environment missing [code: 10090]")
      || cloudflareWorkerMissing("authentication failed [code: 10000]")) {
    throw new Error("Wrangler missing-Worker classification differs");
  }
  if (!cloudflareTransientFailure("A fetch request failed, likely due to a connectivity issue")
      || cloudflareTransientFailure("authentication failed [code: 10000]")) {
    throw new Error("Wrangler transient failure classification differs");
  }
  if (observationUrl("http://127.0.0.1:8787/__workers/account/worker/", "/reset")
      !== "http://127.0.0.1:8787/__workers/account/worker/reset") {
    throw new Error("open-compute differential URL lost its Worker route prefix");
  }
  const source = readFileSync(join(ROOT, "test/conformance/differential.ts"), "utf8");
  if (source.includes("--force") || source.includes("force=true") || source.includes("/client/v4/")) {
    throw new Error("Cloudflare cleanup may force-delete or bypass the pinned Wrangler boundary");
  }
  if (!source.includes('WRANGLER_HIDE_BANNER: "true"')) {
    throw new Error("Wrangler's non-essential update check can escape differential-run cleanup");
  }
  for (const requiredOperation of [
    "ensureCloudflareAbsent", "deployments", "delete", "verifyWranglerAccount",
    "verifyOpenComputeAccount", "createOpenComputeRoute", "readOnlyWrangler", "idempotency-key",
    "restartOpenComputeRuntime", "/__test/runtime/restart",
  ]) {
    if (!source.includes(requiredOperation)) throw new Error(`Cloudflare runner safety operation is missing: ${requiredOperation}`);
  }
  if (!readFileSync(join(ROOT, "test/conformance/adapters.ts"), "utf8").includes("activationDeadline")) {
    throw new Error("Cloudflare activation wait is missing");
  }
  const fixture = (await loadPortableFixtures(join(ROOT, "test/conformance/fixtures")))[0];
  if (fixture === undefined) throw new Error("portable differential fixture is missing");
  const project = openComputeProject(
    fixture,
    name,
    "http://127.0.0.1:8787/",
    "019c0000-0000-7000-8000-000000000001",
  );
  if (!Array.isArray(project.services) || project.services.length !== 0) {
    throw new Error("open-compute differential project does not use the current service declaration schema");
  }
  if (project.main !== "src/index.ts" || project.tsconfig !== "tsconfig.json") {
    throw new Error("open-compute differential project is not self-contained");
  }
}

const checks: Record<CaseId, () => void | Promise<void>> = {
  "baseline-identity": baselineIdentity,
  "catalog-schema": catalogSchema,
  "capability-catalog-bijection": capabilityCatalogBijection,
  "case-registry-mapping": caseRegistryMapping,
  "deviation-bijection": deviationBijection,
  "compatibility-coverage": compatibilityCoverage,
  "public-types-surface": publicTypesSurface,
  "unsupported-config-rejection": unsupportedConfigRejection,
  "portable-fixture-inventory": portableFixtureInventory,
  "cloudflare-runner-safety": cloudflareRunnerSafety,
};

const args = process.argv.slice(2);
if (args.length === 1 && args[0] === "--list") {
  process.stdout.write(`${JSON.stringify({ schemaVersion: 1, cases: CASES })}\n`);
} else {
  const selected: string[] = [];
  for (let index = 0; index < args.length; index += 2) {
    if (args[index] !== "--case" || args[index + 1] === undefined) throw new Error("use --case <id>");
    selected.push(args[index + 1]!);
  }
  const requested = selected.length ? selected : [...CASES];
  if (new Set(requested).size !== requested.length || requested.some(id => !CASES.includes(id as CaseId))) {
    throw new Error("unknown or duplicate conformance case");
  }
  const results: { id: string; status: "passed" | "failed"; error?: string }[] = [];
  for (const id of requested) {
    try {
      await checks[id as CaseId]();
      results.push({ id, status: "passed" });
    } catch (error) {
      results.push({ id, status: "failed", error: error instanceof Error ? error.message : "conformance check failed" });
    }
  }
  const status = results.every(result => result.status === "passed") ? "passed" : "failed";
  process.stdout.write(`${JSON.stringify({ schemaVersion: 1, status, cases: results })}\n`);
  if (status === "failed") process.exitCode = 1;
}
