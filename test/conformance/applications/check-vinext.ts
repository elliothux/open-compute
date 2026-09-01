import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { existsSync, lstatSync, readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

type JsonRecord = Record<string, unknown>;

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const APPLICATION = "test/applications/vinext";
const MANIFEST = "test/conformance/applications/vinext.json";
const CASES = "test/conformance/applications/vinext-cases.json";
const ORCHESTRATION_CASES = new Set([
  "vinext/build/production-output",
  "vinext/import/framework-output",
  "vinext/import/binding-reconciliation",
  "vinext/deploy/immutable-activation",
  "vinext/cleanup/exact-absence",
]);

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

function json(path: string): JsonRecord {
  return record(JSON.parse(readFileSync(join(ROOT, path), "utf8")), path);
}

function sha256(bytes: string | Buffer): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function digest(path: string): string {
  return sha256(readFileSync(join(ROOT, path)));
}

function fixtureTreeDigest(): string {
  const output = createHash("sha256");
  const names = execFileSync(
    "git",
    ["-c", "core.excludesFile=/dev/null", "ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", APPLICATION],
    { cwd: ROOT },
  ).toString("utf8").split("\0").filter(Boolean).sort();
  for (const name of names) {
    const path = join(ROOT, name);
    if (!lstatSync(path).isFile()) throw new Error(`fixture entry is not a regular file: ${name}`);
    output.update(relative(join(ROOT, APPLICATION), path));
    output.update("\0");
    output.update(readFileSync(path));
    output.update("\0");
  }
  return output.digest("hex");
}

function sameStrings(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && [...left].sort().every((value, index) => value === [...right].sort()[index]);
}

function validatePackages(manifest: JsonRecord): void {
  const expected = record(manifest.packages, "manifest.packages");
  const fixture = json(`${APPLICATION}/package.json`);
  const declared = {
    ...record(fixture.dependencies, "fixture.dependencies"),
    ...record(fixture.devDependencies, "fixture.devDependencies"),
  };
  for (const [name, version] of Object.entries(expected)) {
    if (declared[name] !== version || typeof version !== "string" || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
      throw new Error(`fixture package identity drift: ${name}`);
    }
  }
  for (const version of Object.values(declared)) {
    if (typeof version !== "string" || version === "latest" || version.includes("*") || version.startsWith("^") || version.startsWith("~")) {
      throw new Error("fixture dependencies must use exact versions");
    }
  }
}

function validateCases(manifest: JsonRecord, matrix: JsonRecord): JsonRecord[] {
  if (matrix.schemaVersion !== 1 || matrix.application !== "vinext") throw new Error("unsupported vinext case matrix");
  const catalogIds = new Set(array(json("test/conformance/catalog.json").contracts, "catalog.contracts")
    .map((item, index) => string(record(item, `catalog.contracts[${index}]`).id, "contract.id")));
  const cases = array(matrix.cases, "matrix.cases").map((item, index) => record(item, `matrix.cases[${index}]`));
  const ids = new Set<string>();
  const selected: string[] = [];
  const optional: string[] = [];
  const excluded: string[] = [];
  for (const entry of cases) {
    const id = string(entry.id, "case.id");
    if (!/^vinext\/[a-z0-9-]+\/[a-z0-9-]+$/.test(id) || ids.has(id)) throw new Error(`duplicate or invalid case id: ${id}`);
    ids.add(id);
    if (!['build', 'production-http', 'browser'].includes(string(entry.mode, `${id}.mode`))) throw new Error(`${id}: invalid mode`);
    if (!['ONCE', 'TIMING'].includes(string(entry.repetition, `${id}.repetition`))) throw new Error(`${id}: invalid repetition`);
    if (!['platform-contract', 'application-behavior', 'toolchain-only'].includes(string(entry.classification, `${id}.classification`))) {
      throw new Error(`${id}: invalid classification`);
    }
    const fixture = string(entry.fixture, `${id}.fixture`);
    if (!fixture.startsWith(`${APPLICATION}/`) && fixture !== APPLICATION) throw new Error(`${id}: fixture escapes the application`);
    if (!existsSync(join(ROOT, fixture))) throw new Error(`${id}: fixture path is missing`);
    if (!strings(entry.requestSequence, `${id}.requestSequence`).length) throw new Error(`${id}: request sequence is empty`);
    string(entry.observation, `${id}.observation`);
    string(entry.cloudflareSupportCategory, `${id}.cloudflareSupportCategory`);
    string(entry.selectionReason, `${id}.selectionReason`);
    for (const contract of strings(entry.contracts, `${id}.contracts`)) {
      if (!catalogIds.has(contract)) throw new Error(`${id}: unknown contract ${contract}`);
    }
    const selection = string(entry.selection, `${id}.selection`);
    if (selection === "mandatory") selected.push(id);
    else if (selection === "optional-partial") optional.push(id);
    else if (selection === "excluded") excluded.push(id);
    else throw new Error(`${id}: invalid selection`);
  }
  if (!sameStrings(strings(manifest.selectedCases, "manifest.selectedCases"), selected)
      || !sameStrings(strings(manifest.optionalCases, "manifest.optionalCases"), optional)
      || !sameStrings(strings(manifest.excludedCases, "manifest.excludedCases"), excluded)) {
    throw new Error("manifest and case selections differ");
  }
  return cases;
}

function validateBrowser(manifest: JsonRecord): void {
  const browser = record(manifest.browser, "manifest.browser");
  if (string(browser.name, "browser.name") !== "chromium") {
    throw new Error("fixed browser name is missing or changed");
  }
  const fixtureRequire = createRequire(join(ROOT, APPLICATION, "package.json"));
  const playwrightPackage = record(fixtureRequire("@playwright/test/package.json"), "@playwright/test/package.json");
  if (string(playwrightPackage.version, "@playwright/test.version") !== string(browser.playwrightVersion, "browser.playwrightVersion")) {
    throw new Error("Playwright package version is missing or changed");
  }
  const playwrightRequire = createRequire(fixtureRequire.resolve("@playwright/test/package.json"));
  const browsersManifest = record(
    createRequire(playwrightRequire.resolve("playwright-core/package.json"))("./browsers.json"),
    "playwright-core/browsers.json",
  );
  const chromium = array(browsersManifest.browsers, "browsers.json.browsers")
    .map((item, index) => record(item, `browsers.json.browsers[${index}]`))
    .find(entry => entry.name === "chromium");
  if (!chromium) throw new Error("Playwright Chromium inventory is missing");
  const revision = typeof chromium.revision === "number" ? String(chromium.revision) : string(chromium.revision, "chromium.revision");
  if (revision !== string(browser.playwrightRevision, "browser.playwrightRevision")) {
    throw new Error("Playwright Chromium revision is missing or changed");
  }
  if (string(chromium.browserVersion, "chromium.browserVersion") !== string(browser.browserVersion, "browser.browserVersion")) {
    throw new Error("Playwright Chromium browserVersion is missing or changed");
  }
}

function validateRunner(cases: readonly JsonRecord[]): void {
  const raw = execFileSync("bun", [`${APPLICATION}/tests/qualification.ts`, "--list"], {
    cwd: ROOT,
    env: { PATH: process.env.PATH },
  }).toString("utf8");
  const report = record(JSON.parse(raw), "qualification --list");
  if (report.schemaVersion !== 1) throw new Error("unsupported qualification runner inventory");
  const expected = cases
    .filter(entry => entry.selection === "mandatory" && !ORCHESTRATION_CASES.has(string(entry.id, "case.id")))
    .map(entry => string(entry.id, "case.id"));
  if (!sameStrings(strings(report.cases, "qualification cases"), expected)) {
    throw new Error("qualification runner and selected runtime cases differ");
  }
}

function validateGo(manifest: JsonRecord): void {
  const status = record(manifest.p4Status, "manifest.p4Status");
  if (status.verdict !== "go" || status.cloudflareSemantics !== "worker-version-and-deployment") {
    throw new Error("P4 Cloudflare-aligned verdict is not frozen");
  }
  const evidence = record(status.evidence, "manifest.p4Status.evidence");
  if (evidence.sourceBuildReproducibility !== "non-blocking-upstream-deviation"
      || evidence.localRuntimeCases !== "15/15"
      || evidence.cloudflareRuntimeCases !== "15/15"
      || evidence.cloudflareCleanup !== "worker-absent"
      || evidence.localCleanup !== "worker-route-and-processes-absent") {
    throw new Error("P4 differential or cleanup evidence is incomplete");
  }
  const first = string(evidence.firstSourceInventorySha256, "first source inventory digest");
  const second = string(evidence.secondSourceInventorySha256, "second source inventory digest");
  if (first === second) throw new Error("source-build deviation evidence was lost");
  if (evidence.importedModuleCount !== 79 || evidence.wranglerModuleCount !== 79
      || evidence.importedModuleNamesSha256 !== evidence.wranglerModuleNamesSha256) {
    throw new Error("frozen artifact importer does not match Wrangler module discovery");
  }
  for (const key of ["localBundleSha256", "cloudflareVersionId", "cloudflareDeploymentId"] as const) {
    string(evidence[key], `manifest.p4Status.evidence.${key}`);
  }
}

function main(): void {
  if (process.argv.slice(2).join(" ") !== "--list") throw new Error("use --list");
  const manifest = json(MANIFEST);
  const matrix = json(CASES);
  if (manifest.schemaVersion !== 1 || manifest.application !== "vinext") throw new Error("unsupported vinext manifest");
  if (digest("bun.lock") !== string(manifest.rootLockSha256, "manifest.rootLockSha256")) throw new Error("root lock digest drift");
  if (fixtureTreeDigest() !== string(manifest.fixtureTreeSha256, "manifest.fixtureTreeSha256")) throw new Error("fixture tree digest drift");
  if (digest(CASES) !== string(manifest.casesSha256, "manifest.casesSha256")) throw new Error("case matrix digest drift");
  validatePackages(manifest);
  const cases = validateCases(manifest, matrix);
  validateBrowser(manifest);
  validateRunner(cases);
  validateGo(manifest);
  const selected = cases.filter(entry => entry.selection !== "excluded");
  console.log(JSON.stringify({
    schemaVersion: 1,
    application: "vinext",
    verdict: "go",
    selected: selected.length,
    mandatory: selected.filter(entry => entry.selection === "mandatory").length,
    optional: selected.filter(entry => entry.selection === "optional-partial").length,
    excluded: cases.length - selected.length,
    cases: cases.map(entry => entry.id),
  }));
}

main();
