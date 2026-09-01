import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadProject } from "../../packages/toolchain/src/project.ts";
import { validateCaseEvidence } from "./case-evidence.ts";
import {
  cloudflareDeploymentUrl, cloudflareProject, cloudflareTransientFailure, cloudflareWorkerMissing,
  loadPortableFixtures, observationUrl, openComputeProject,
} from "./adapters.ts";
import { generateInventoryTwice } from "./inventory.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const CASES = [
  "baseline-identity",
  "catalog-schema",
  "capability-catalog-bijection",
  "inventory-generation-drift",
  "inventory-member-evidence",
  "case-registry-mapping",
  "deviation-bijection",
  "compatibility-coverage",
  "public-types-surface",
  "compile-fixtures",
  "conformance-self-tests",
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
function inventory(): JsonRecord { return record(json("share/cloudflare-capabilities.json"), "inventory"); }
function capabilities(): JsonRecord { return record(inventory().products, "inventory.products"); }

function inventoryMembers(): JsonRecord[] {
  const members: JsonRecord[] = [];
  for (const [product, raw] of Object.entries(capabilities())) {
    const capability = record(raw, `capability ${product}`);
    for (const [index, item] of array(capability.members ?? [], `${product}.members`).entries()) {
      members.push(record(item, `${product}.members[${index}]`));
    }
  }
  return members;
}
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
  if (lock.release !== workerd.release || lock.revision !== workerd.revision
      || lock.expectedVersionOutput !== "workerd 2026-08-30") {
    throw new Error("workerd release identity drift");
  }
  const workersTypes = record(value.workersTypes, "workersTypes");
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  if (workersTypes.version !== "5.20260830.1" || lockTypes.version !== workersTypes.version
      || lockTypes.gitHead !== lock.revision
      || workersTypes.lockSha256 !== digest("bun.lock")
      || workersTypes.packageSha256 !== lockTypes.packageSha256
      || workersTypes.astSha256 !== lockTypes.astSha256) {
    throw new Error("workers-types lock identity drift");
  }
  const sdk = record(value.workersSdk, "workersSdk");
  const lockSdk = record(lock.workersSdk, "lock.workersSdk");
  if (sdk.revision !== lockSdk.revision || lockSdk.wranglerVersion !== record(value.wrangler, "wrangler").version
      || lockSdk.vitePluginVersion !== record(value.vitePlugin, "vitePlugin").version
      || !/^[0-9a-f]{40}$/.test(string(sdk.revision, "workersSdk.revision"))
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
    if ("methods" in contract) throw new Error(`${id}: coarse methods lists are forbidden`);
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
  const evidenceIds = new Set<string>();
  for (const [index, raw] of array(catalog().memberEvidence ?? [], "memberEvidence").entries()) {
    const item = record(raw, `memberEvidence[${index}]`);
    const id = string(item.id, `memberEvidence[${index}].id`);
    if (evidenceIds.has(id)) throw new Error(`duplicate memberEvidence id: ${id}`);
    evidenceIds.add(id);
    const status = string(item.status, `${id}.status`);
    if (!["supported", "supported_with_deviation", "blocked"].includes(status)) {
      throw new Error(`${id}: invalid memberEvidence status`);
    }
    const compileCases = strings(item.compileCases ?? [], `${id}.compileCases`);
    const runtimeCases = strings(item.runtimeCases ?? [], `${id}.runtimeCases`);
    if ((status === "supported" || status === "supported_with_deviation")
        && (!compileCases.length || !runtimeCases.length)) {
      throw new Error(`${id}: supported member lacks compile and real-runtime cases`);
    }
    if (status === "blocked" && (compileCases.length || runtimeCases.length)) {
      throw new Error(`${id}: blocked member must not carry evidence cases`);
    }
  }
  for (const [index, raw] of array(catalog().blockedGaps ?? [], "blockedGaps").entries()) {
    const gap = record(raw, `blockedGaps[${index}]`);
    string(gap.id, `blockedGaps[${index}].id`);
    const memberIds = strings(gap.memberIds, `blockedGaps[${index}].memberIds`);
    if (!memberIds.length || new Set(memberIds).size !== memberIds.length) {
      throw new Error(`${gap.id}: blocked gap has no exact member IDs or contains duplicates`);
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
    if ("methods" in capability) throw new Error(`${product}: coarse methods lists are forbidden`);
    const capabilityDeviations = strings(capability.deviations ?? [], `${product}.deviations`).sort();
    const contractDeviations = product === contract.product ? strings(contract.deviations, `${product}.contractDeviations`).sort() : [];
    if (capabilityDeviations.join("\0") !== contractDeviations.join("\0")) throw new Error(`${product}: deviations differ`);
  }
}

async function inventoryGenerationDrift(): Promise<void> {
  const { encoded } = await generateInventoryTwice();
  const committed = readFileSync(join(ROOT, "share/cloudflare-capabilities.json"), "utf8");
  if (encoded !== committed) throw new Error("share/cloudflare-capabilities.json drifted from generated inventory");
  const value = inventory();
  if (value.schema_version !== 1) throw new Error("inventory schema_version must be 1");
  const source = record(value.source, "inventory.source");
  const lock = record(json("packages/runtime/workerd.lock.json"), "workerd lock");
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  if (string(source.workers_types_version, "source.workers_types_version") !== string(lockTypes.version, "lock.workersTypes.version")
      || string(source.git_head, "source.git_head") !== string(lockTypes.gitHead, "lock.workersTypes.gitHead")
      || string(source.package_sha256, "source.package_sha256") !== string(lockTypes.packageSha256, "lock.workersTypes.packageSha256")
      || string(source.ast_sha256, "source.ast_sha256") !== string(lockTypes.astSha256, "lock.workersTypes.astSha256")) {
    throw new Error("inventory source identity does not match the formal workers-types pin");
  }
}

function inventoryMemberEvidence(): void {
  const members = inventoryMembers();
  const byId = new Map<string, JsonRecord>();
  for (const member of members) {
    const id = string(member.id, "member.id");
    if (byId.has(id)) throw new Error(`duplicate inventory member: ${id}`);
    byId.set(id, member);
    const status = string(member.status, `${id}.status`);
    if (status === "unsupported") throw new Error(`${id}: unsupported is reserved for non-target products`);
    const compileCases = strings(member.compile_cases ?? [], `${id}.compile_cases`);
    const runtimeCases = strings(member.runtime_cases ?? [], `${id}.runtime_cases`);
    if ((status === "supported" || status === "supported_with_deviation")
        && (!compileCases.length || !runtimeCases.length)) {
      throw new Error(`${id}: supported member lacks compile and real-runtime cases`);
    }
    if (status === "blocked" && (compileCases.length || runtimeCases.length)) {
      throw new Error(`${id}: blocked member must not carry evidence cases`);
    }
  }
  const evidenceIds = new Set<string>();
  for (const [index, raw] of array(catalog().memberEvidence ?? [], "memberEvidence").entries()) {
    const item = record(raw, `memberEvidence[${index}]`);
    const id = string(item.id, `memberEvidence[${index}].id`);
    const member = byId.get(id);
    if (member === undefined) throw new Error(`stale memberEvidence id: ${id}`);
    evidenceIds.add(id);
    if (string(member.status, `${id}.status`) !== string(item.status, `${id}.evidenceStatus`)) {
      throw new Error(`${id}: inventory status does not match memberEvidence`);
    }
  }
  for (const member of members) {
    const status = string(member.status, "member.status");
    const id = string(member.id, "member.id");
    if ((status === "supported" || status === "supported_with_deviation") && !evidenceIds.has(id)) {
      throw new Error(`${id}: supported member is missing from memberEvidence`);
    }
  }
  const coveredBlocked = new Set<string>();
  for (const [index, raw] of array(catalog().blockedGaps ?? [], "blockedGaps").entries()) {
    const gap = record(raw, `blockedGaps[${index}]`);
    const gapId = string(gap.id, `blockedGaps[${index}].id`);
    for (const id of strings(gap.memberIds, `${gapId}.memberIds`)) {
      const member = byId.get(id);
      if (member === undefined) throw new Error(`${gapId}: stale blocked member ID: ${id}`);
      if (member.status !== "blocked") throw new Error(`${gapId}: ${id} must remain blocked`);
      if (coveredBlocked.has(id)) throw new Error(`${id}: blocked member has more than one gap owner`);
      coveredBlocked.add(id);
    }
  }
  for (const member of members) {
    const id = string(member.id, "member.id");
    if (member.status === "blocked" && !coveredBlocked.has(id)) {
      throw new Error(`${id}: blocked member has no explicit gap owner`);
    }
  }
}

function caseRegistryMapping(): void {
  const output = execFileSync("python3", [join(ROOT, "test/gate_cases.py"), "--json"], {
    cwd: ROOT,
    encoding: "utf8",
    env: { PATH: process.env.PATH },
    timeout: 10_000,
  });
  validateCaseEvidence(ROOT, catalog(), JSON.parse(output));
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
  const lock = record(json("packages/runtime/workerd.lock.json"), "workerd lock");
  const date = string(lock.effectiveCompatibilityDate, "lock.effectiveCompatibilityDate");
  if (string(baseline().effectiveCompatibilityDate, "baseline.effectiveCompatibilityDate") !== date) {
    throw new Error("baseline effective compatibility date does not match the formal lock");
  }
  for (const contract of contracts()) {
    if (contract.compatibility !== undefined) {
      throw new Error(`${contract.id}: catalog must not carry tenant compatibility selectors`);
    }
  }
}

function typesAstEnv(): NodeJS.ProcessEnv {
  const tmp = join(ROOT, ".temp/bun-tmp");
  const transpile = join(ROOT, ".temp/bun-transpile");
  mkdirSync(tmp, { recursive: true });
  mkdirSync(transpile, { recursive: true });
  return {
    ...process.env,
    TMPDIR: tmp,
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: transpile,
  };
}

function fingerprintFile(path: string): { sha256: string; statements: number; lines: number } {
  const output = execFileSync(process.execPath, [join(ROOT, "test/conformance/types-ast.ts"), "fingerprint", path], {
    cwd: ROOT, encoding: "utf8", env: typesAstEnv(), timeout: 60_000, maxBuffer: 1024 * 1024,
  });
  const value = record(JSON.parse(output), `fingerprint ${path}`);
  if (typeof value.statements !== "number" || typeof value.lines !== "number") {
    throw new Error(`fingerprint ${path} is malformed`);
  }
  return { sha256: string(value.sha256, "fingerprint.sha256"), statements: value.statements, lines: value.lines };
}

async function publicTypesSurface(): Promise<void> {
  const lock = record(json("packages/runtime/workerd.lock.json"), "workerd lock");
  const lockTypes = record(lock.workersTypes, "lock.workersTypes");
  const baselineTypes = record(baseline().workersTypes, "baseline.workersTypes");
  const workersTypesRoot = dirname(createRequire(join(ROOT, "packages/types/package.json"))
    .resolve("@cloudflare/workers-types/package.json"));
  const packageJson = record(json(relative(ROOT, join(workersTypesRoot, "package.json"))), "workers-types package");
  if (packageJson.version !== lockTypes.version || packageJson.version !== "5.20260830.1") {
    throw new Error("installed @cloudflare/workers-types version drift");
  }
  const installedPath = join(workersTypesRoot, "index.d.ts");
  const snapshotPath = join(ROOT, "references/workerd/types/generated-snapshot/index.d.ts");
  const installed = readFileSync(installedPath);
  const indexSha256 = string(baselineTypes.indexSha256, "workersTypes.indexSha256");
  if (sha256(installed) !== indexSha256) {
    throw new Error("workers-types index digest drift");
  }
  const installedAst = fingerprintFile(installedPath);
  if (existsSync(snapshotPath)) {
    const snapshot = readFileSync(snapshotPath);
    if (sha256(snapshot) !== indexSha256) {
      throw new Error("workers-types index digest drift");
    }
    if (!installed.equals(snapshot)) {
      throw new Error("npm workers-types and workerd generated snapshot are not byte-identical");
    }
    const snapshotAst = fingerprintFile(snapshotPath);
    if (installedAst.sha256 !== snapshotAst.sha256) {
      throw new Error("npm workers-types and workerd generated snapshot are not structurally identical");
    }
  }
  if (installedAst.sha256 !== string(lockTypes.astSha256, "lock.workersTypes.astSha256")
      || installedAst.sha256 !== string(baselineTypes.astSha256, "baseline.workersTypes.astSha256")) {
    throw new Error("workers-types AST digest drift");
  }
  if (installedAst.lines !== 17525 || installedAst.statements < 100) {
    throw new Error("upstream stable declaration is incomplete");
  }
  execFileSync(process.execPath, [join(ROOT, "test/conformance/types-ast.ts"), "thin-bridge", join(ROOT, "packages/types/index.d.ts")], {
    cwd: ROOT, encoding: "utf8", env: typesAstEnv(), timeout: 60_000,
  });
  const example = readFileSync(join(ROOT, "examples/hello-worker/tsconfig.json"), "utf8");
  if (!example.includes("@open-compute/workers-types") || example.includes("workers-types/experimental")) {
    throw new Error("example does not consume the pinned stable type surface");
  }
  const fixtures = readFileSync(join(ROOT, "test/conformance/fixtures/tsconfig.json"), "utf8");
  if (!fixtures.includes("@open-compute/workers-types") || fixtures.includes("workers-types/experimental")) {
    throw new Error("tenant fixtures do not consume the pinned stable type surface");
  }
}

function compileFixtures(): void {
  execFileSync(join(ROOT, "node_modules/.bin/tsc"), [
    "--project", join(ROOT, "test/conformance/fixtures/tsconfig.json"),
    "--noEmit", "--pretty", "false",
  ], { cwd: ROOT, encoding: "utf8", timeout: 120_000, maxBuffer: 4 * 1024 * 1024 });
}

function conformanceSelfTests(): void {
  try {
    execFileSync("node", [
      "--test",
      join(ROOT, "test/conformance/adapters.test.mjs"),
      join(ROOT, "test/conformance/case-evidence.test.mjs"),
      join(ROOT, "test/conformance/inventory.test.mjs"),
    ], { cwd: ROOT, encoding: "utf8", env: typesAstEnv(), timeout: 120_000, maxBuffer: 4 * 1024 * 1024 });
  } catch (error) {
    const failure = error as { message?: unknown; stderr?: unknown; stdout?: unknown };
    throw new Error([failure.message, failure.stderr, failure.stdout].filter(Boolean).join("\n"));
  }
}

async function unsupportedConfigRejection(): Promise<void> {
  const directory = mkdtempSync(join(tmpdir(), "open-compute-p3-contract-"));
  try {
    for (const type of ["analytics_engine", "ai", "browser", "vectorize", "hyperdrive", "mtls_certificate", "rate_limit", "worker_loader"]) {
      const path = join(directory, `${type}.json`);
      writeFileSync(path, JSON.stringify({
        main: "worker.ts", name: "unsupported-probe", tsconfig: "tsconfig.json",
        vars: {}, secrets: {},
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
  const source = [
    readFileSync(join(ROOT, "test/conformance/differential.ts"), "utf8"),
    readFileSync(join(ROOT, "test/conformance/differential-product-resources.ts"), "utf8"),
  ].join("\n");
  if (source.includes("--force") || source.includes("/client/v4/")) {
    throw new Error("Cloudflare cleanup may force-delete or bypass the pinned Wrangler boundary");
  }
  if (!source.includes('WRANGLER_HIDE_BANNER: "true"')) {
    throw new Error("Wrangler's non-essential update check can escape differential-run cleanup");
  }
  for (const requiredOperation of [
    "ensureCloudflareAbsent", "deployments", "delete", "verifyWranglerAccount",
    "verifyOpenComputeAccount", "ensureOpenComputeAbsent", "createOpenComputeRoute",
    "cleanupOpenComputeRoute", "recordOwnership", "readOnlyWrangler", "idempotency-key",
    "ensureCloudflareKvAbsent", "createCloudflareKv", "cleanupCloudflareKv",
    "ensureOpenComputeKvAbsent", "createOpenComputeKv", "cleanupOpenComputeKv",
    "ensureCloudflareD1Absent", "createCloudflareD1", "cleanupCloudflareD1",
    "ensureOpenComputeD1Absent", "createOpenComputeD1", "cleanupOpenComputeD1",
    "ensureCloudflareR2Absent", "createCloudflareR2", "cleanupCloudflareR2",
    "ensureOpenComputeR2Absent", "createOpenComputeR2", "cleanupOpenComputeR2",
    "ensureCloudflareQueueAbsent", "createCloudflareQueue", "cleanupCloudflareQueue",
    "ensureOpenComputeQueueAbsent", "createOpenComputeQueue", "cleanupOpenComputeQueue",
    "ensureOpenComputeDurableObjectNamespaceAbsent", "createOpenComputeDurableObjectNamespace",
    "cleanupOpenComputeDurableObjectNamespace", "ensureCloudflareWorkflowAbsent",
    "verifyCloudflareWorkflowCreated", "cleanupCloudflareWorkflow", "ensureOpenComputeWorkflowAbsent",
    "createOpenComputeWorkflow", "activateOpenComputeWorkflowVersion", "cleanupOpenComputeWorkflow",
    "--skip-confirmation",
  ]) {
    if (!source.includes(requiredOperation)) throw new Error(`Cloudflare runner safety operation is missing: ${requiredOperation}`);
  }
  if (!source.includes('["delete", "--name", name, "--config", config]')
      || !source.includes("restartOpenComputeRuntime") || !source.includes("/__test/runtime/restart")
      || !source.includes("OPEN_COMPUTE_TEST_RUNTIME_RESTART_ACK")) {
    throw new Error("differential cleanup is not exact or lacks the guarded test-runtime restart required by deployment retention");
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
  const kvFixture = (await loadPortableFixtures(join(ROOT, "test/conformance/fixtures")))
    .find(item => item.id === "kv/portable/namespace");
  if (kvFixture === undefined) throw new Error("portable KV differential fixture is missing");
  const kvProject = openComputeProject(
    kvFixture,
    name,
    "http://127.0.0.1:8787/",
    "019c0000-0000-7000-8000-000000000001",
    { KV: "019c0000-0000-7000-8000-000000000002" },
  );
  if (JSON.stringify(kvProject.bindings) !== JSON.stringify({
    KV: { type: "kv_namespace", id: "019c0000-0000-7000-8000-000000000002" },
  })) throw new Error("portable KV binding does not use an exact owned resource identity");
  const d1Fixture = (await loadPortableFixtures(join(ROOT, "test/conformance/fixtures")))
    .find(item => item.id === "d1/portable/database");
  if (d1Fixture === undefined) throw new Error("portable D1 differential fixture is missing");
  const d1Ids = {
    DB: "019c0000-0000-7000-8000-000000000003",
    OTHER: "019c0000-0000-7000-8000-000000000004",
  };
  const d1Project = openComputeProject(
    d1Fixture,
    name,
    "http://127.0.0.1:8787/",
    "019c0000-0000-7000-8000-000000000001",
    d1Ids,
  );
  if (JSON.stringify(d1Project.bindings) !== JSON.stringify({
    DB: { type: "d1_database", id: d1Ids.DB },
    OTHER: { type: "d1_database", id: d1Ids.OTHER },
  })) throw new Error("portable D1 binding does not use exact owned resource identities");
  const cfD1 = cloudflareProject(
    d1Fixture,
    name,
    "0123456789abcdef0123456789abcdef",
    { DB: "11111111-1111-4111-8111-111111111111", OTHER: "22222222-2222-4222-8222-222222222222" },
    { DB: `${name}-d1-0`, OTHER: `${name}-d1-1` },
  );
  if (!Array.isArray(cfD1.d1_databases) || cfD1.d1_databases.length !== 2
      || !Array.isArray(cfD1.kv_namespaces) || cfD1.kv_namespaces.length !== 0) {
    throw new Error("portable Cloudflare D1 project does not bind only exact owned databases");
  }
}

const checks: Record<CaseId, () => void | Promise<void>> = {
  "baseline-identity": baselineIdentity,
  "catalog-schema": catalogSchema,
  "capability-catalog-bijection": capabilityCatalogBijection,
  "inventory-generation-drift": inventoryGenerationDrift,
  "inventory-member-evidence": inventoryMemberEvidence,
  "case-registry-mapping": caseRegistryMapping,
  "deviation-bijection": deviationBijection,
  "compatibility-coverage": compatibilityCoverage,
  "public-types-surface": publicTypesSurface,
  "compile-fixtures": compileFixtures,
  "conformance-self-tests": conformanceSelfTests,
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
