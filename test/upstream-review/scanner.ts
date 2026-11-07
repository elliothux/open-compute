// Cloudflare upstream refresh scanner: read-only discovery and three-way
// classification of the OpenAPI snapshot, official TypeScript SDK, and
// Cf identities against the repository pin.
//
// The scanner is shared by local review and the scheduled workflow. It only
// ever writes downloads and reports under `.temp/upstream-review/`, never
// executes package lifecycle scripts, and holds no publish credentials.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { scanOfficialPackage } from "../../packages/sdk/scripts/sdk-scan.ts";
import {
  buildSubset,
  officialCfSchema,
  prettierJson,
} from "../conformance/p6-contract.mjs";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const REVIEW_ROOT = resolve(REPO_ROOT, ".temp/upstream-review");
const MANIFEST_PATH = join(
  REPO_ROOT,
  "openapi/cloudflare-subset-manifest.json",
);
const SUBSET_PATH = join(REPO_ROOT, "openapi/cloudflare-v4-subset.json");
const LOCK_PATH = join(
  REPO_ROOT,
  "openapi/upstream/cloudflare-openapi.lock.json",
);
const CAPABILITY_SOURCE_PATH = join(
  REPO_ROOT,
  "openapi/p6-capability-source.json",
);

export type Classification = "unchanged" | "ready" | "blocked" | "breaking";

export interface NpmIdentity {
  version: string;
  npmShasum: string;
  npmIntegrity: string;
}

export interface CfEvidence {
  packageSha256: string;
  packageJsonSha256: string;
  configSchemaSha256: string;
  cliSha256: string;
  missingConfigFields: string[];
  missingBindings: string[];
  missingCommands: string[];
  unknownDifferences: string[];
}

export interface UpstreamReport {
  schemaVersion: 1;
  classification: Classification;
  generatedAtUtc: string;
  baseline: {
    openapiRevision: string;
    openapiSha256: string;
    cloudflareSdkVersion: string;
    cfVersion: string;
  };
  candidate: {
    openapiRevision: string | null;
    openapiSha256: string | null;
    cloudflareSdk: NpmIdentity | null;
    cf: NpmIdentity | null;
    cfEvidence: CfEvidence | null;
  };
  selectedOperations: number;
  changedOperations: string[];
  /** Selected operations whose path only renamed parameters, by baseline id. */
  renamedOperations: Array<{ from: string; to: string }>;
  removedOperations: string[];
  unmappedOperations: string[];
  waitingOn: string[];
  reasons: string[];
}

export interface ScanInput {
  baseline: {
    openapiRevision: string;
    openapiSha256: string;
    cloudflareSdkVersion: string;
    cfVersion: string;
  };
  candidateSchema: { revision: string; bytes: Buffer };
  candidateCloudflare: NpmIdentity;
  candidateCf: NpmIdentity;
  candidateCfEvidence: CfEvidence;
  /**
   * (METHOD normalized-path) routes implemented by the candidate official
   * SDK, extracted statically from its published resources.
   */
  candidateSdkRoutes: Map<string, string[]>;
  /** Selection manifest used to build the candidate subset. */
  manifest: unknown;
  /** Selected-operation keys the manifest excludes from the SDK surface. */
  sdkExcludedOperations: string[];
  /** Operations exported by the committed baseline SDK surface. */
  baselineMappedOperations: string[];
  /** Operation digests from the committed baseline subset inventory. */
  baselineInventory: Array<{ id: string; operationSha256: string }>;
  /** Operation keys the baseline manifest selected. */
  selectedOperations: string[];
}

export function sha256Hex(value: Buffer | string): string {
  return createHash("sha256").update(value).digest("hex");
}

function normalize(path: string): string {
  return path.replaceAll(/\{[^}]*\}/g, "{}");
}

const HTTP_METHODS = new Set([
  "get",
  "put",
  "post",
  "delete",
  "options",
  "head",
  "patch",
  "trace",
]);

/**
 * Match each selected operation to the candidate by method and
 * parameter-normalized path. Path parameter names are not on the wire, so a
 * unique normalized match is a rename, not a removal.
 */
function matchSelectedOperations(
  selected: string[],
  candidatePaths: Record<string, Record<string, unknown>>,
): { renamed: Map<string, string>; removed: string[] } {
  const byShape = new Map<string, string[]>();
  for (const [path, item] of Object.entries(candidatePaths)) {
    for (const method of Object.keys(item)) {
      if (!HTTP_METHODS.has(method)) continue;
      const shape = `${method.toUpperCase()} ${normalize(path)}`;
      byShape.set(shape, [
        ...(byShape.get(shape) ?? []),
        `${method.toUpperCase()} ${path}`,
      ]);
    }
  }
  const renamed = new Map<string, string>();
  const removed: string[] = [];
  for (const id of selected) {
    const separator = id.indexOf(" ");
    const method = id.slice(0, separator);
    const path = id.slice(separator + 1);
    if (candidatePaths[path]?.[method.toLowerCase()] !== undefined) continue;
    const matches = byShape.get(`${method} ${normalize(path)}`) ?? [];
    if (matches.length === 1 && matches[0] !== undefined)
      renamed.set(id, matches[0]);
    else removed.push(id);
  }
  return { renamed, removed };
}

/** Replace every exact operation-id string in the manifest with its renamed id. */
function renameManifestOperations(
  value: unknown,
  renamed: Map<string, string>,
): unknown {
  if (typeof value === "string") return renamed.get(value) ?? value;
  if (Array.isArray(value))
    return value.map((item) => renameManifestOperations(item, renamed));
  if (value !== null && typeof value === "object")
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [
        key,
        renameManifestOperations(item, renamed),
      ]),
    );
  return value;
}

export function classify(input: ScanInput): UpstreamReport {
  const reasons: string[] = [];
  const waitingOn: string[] = [];
  const candidateSchema = JSON.parse(
    input.candidateSchema.bytes.toString("utf8"),
  );
  const candidatePaths = (candidateSchema.paths ?? {}) as Record<
    string,
    Record<string, unknown>
  >;
  const lostOperations: string[] = [];
  const { renamed, removed: removedOperations } = matchSelectedOperations(
    input.selectedOperations,
    candidatePaths,
  );
  const renamedOperations = [...renamed]
    .map(([from, to]) => ({ from, to }))
    .sort((left, right) => left.from.localeCompare(right.from));
  const sdkExcluded = new Set(
    input.sdkExcludedOperations.map((id) => renamed.get(id) ?? id),
  );
  const changedOperations: string[] = [];
  if (removedOperations.length === 0) {
    const candidate = buildSubset(
      candidateSchema,
      renameManifestOperations(input.manifest, renamed),
      input.candidateSchema.revision,
      sha256Hex(input.candidateSchema.bytes),
    );
    const candidateDigests = new Map(
      (
        candidate["x-open-compute-operation-inventory"] as Array<{
          id: string;
          operationSha256: string;
        }>
      ).map((entry) => [entry.id, entry.operationSha256]),
    );
    const baselineDigests = new Map(
      input.baselineInventory.map((entry) => [entry.id, entry.operationSha256]),
    );
    for (const id of input.selectedOperations) {
      const candidateId = renamed.get(id) ?? id;
      const digest = candidateDigests.get(candidateId);
      if (digest === undefined) continue;
      if (baselineDigests.get(id) !== digest || candidateId !== id)
        changedOperations.push(candidateId);
    }
    for (const entry of candidate[
      "x-open-compute-operation-inventory"
    ] as Array<{
      id: string;
      method: string;
      path: string;
      status: string;
    }>) {
      if (
        entry.status !== "supported" &&
        entry.status !== "supported_with_deviation"
      )
        continue;
      if (sdkExcluded.has(entry.id)) continue;
      const route = `${entry.method} ${normalize(entry.path)}`;
      if (!input.candidateSdkRoutes.has(route)) lostOperations.push(entry.id);
    }
    lostOperations.sort();
  }
  changedOperations.sort();
  const unmappedOperations = [...lostOperations];

  const schemaChanged =
    input.candidateSchema.revision !== input.baseline.openapiRevision ||
    changedOperations.length > 0 ||
    removedOperations.length > 0;
  const sdkChanged =
    input.candidateCloudflare.version !== input.baseline.cloudflareSdkVersion;
  const cfChanged = input.candidateCf.version !== input.baseline.cfVersion;
  const cfBreaking = [
    ...input.candidateCfEvidence.missingConfigFields.map(
      (field) => `config field ${field}`,
    ),
    ...input.candidateCfEvidence.missingBindings.map(
      (binding) => `binding ${binding}`,
    ),
    ...input.candidateCfEvidence.missingCommands.map(
      (command) => `command ${command}`,
    ),
  ];
  const routesMoved =
    removedOperations.length > 0 || changedOperations.length > 0;

  let classification: Classification;
  if (
    !schemaChanged &&
    !sdkChanged &&
    !cfChanged &&
    unmappedOperations.length === 0
  ) {
    classification = "unchanged";
  } else if (removedOperations.length > 0) {
    if (renamedOperations.length > 0)
      reasons.push(
        `selected operations renamed path parameters: ${renamedOperations.map(({ from, to }) => `${from} -> ${to}`).join(", ")}`,
      );
    reasons.push(
      `selected operations disappeared from the upstream snapshot: ${removedOperations.join(", ")}`,
    );
    classification = "breaking";
  } else if (lostOperations.length > 0) {
    // Operations exported by the current surface vanished from the candidate
    // official SDK closure.
    reasons.push(
      `the candidate official SDK no longer implements mapped operations: ${lostOperations.join(", ")}`,
    );
    classification = "breaking";
  } else if (cfBreaking.length > 0) {
    reasons.push(
      `the candidate Cf removed selected surface: ${cfBreaking.join(", ")}`,
    );
    classification = "breaking";
  } else {
    if (renamedOperations.length > 0)
      reasons.push(
        `selected operations renamed path parameters: ${renamedOperations.map(({ from, to }) => `${from} -> ${to}`).join(", ")}`,
      );
    if (unmappedOperations.length > 0) {
      reasons.push(
        "manifest-excluded operations remain unimplemented by the candidate official SDK; the exclusion stays authoritative",
      );
    }
    if (input.candidateCfEvidence.unknownDifferences.length > 0) {
      reasons.push(
        `candidate Cf evidence could not be interpreted: ${input.candidateCfEvidence.unknownDifferences.join(", ")}`,
      );
      waitingOn.push("Cf selected-surface review");
      classification = "blocked";
    } else if (schemaChanged && !sdkChanged && routesMoved) {
      reasons.push(
        "schema snapshot moved but the published official SDK did not; changed operations are not proven against a newer published SDK",
      );
      waitingOn.push("official cloudflare SDK stable release");
      classification = "blocked";
    } else {
      if (schemaChanged) {
        reasons.push(
          `schema snapshot revision ${input.candidateSchema.revision} re-selects ${changedOperations.length} changed operations`,
        );
      }
      if (sdkChanged) {
        reasons.push(
          `official SDK candidate ${input.candidateCloudflare.version} implements the selected contract closure`,
        );
      }
      if (cfChanged) {
        reasons.push(
          `Cf candidate ${input.candidateCf.version} preserves the selected config, binding, and command surface`,
        );
      }
      classification = "ready";
    }
  }

  return {
    schemaVersion: 1,
    classification,
    generatedAtUtc: new Date().toISOString(),
    baseline: {
      openapiRevision: input.baseline.openapiRevision,
      openapiSha256: input.baseline.openapiSha256,
      cloudflareSdkVersion: input.baseline.cloudflareSdkVersion,
      cfVersion: input.baseline.cfVersion,
    },
    candidate: {
      openapiRevision: input.candidateSchema.revision,
      openapiSha256: sha256Hex(input.candidateSchema.bytes),
      cloudflareSdk: input.candidateCloudflare,
      cf: input.candidateCf,
      cfEvidence: input.candidateCfEvidence,
    },
    selectedOperations: input.selectedOperations.length,
    changedOperations,
    renamedOperations,
    removedOperations,
    unmappedOperations,
    waitingOn,
    reasons,
  };
}

interface LockBaseline {
  revision: string;
  sha256: string;
  cloudflareSdk: { version: string };
  cf: { version: string; packageSha256: string };
}

interface ExtractedPackage {
  root: string;
  packageSha256: string;
  packageJsonSha256: string;
}

function loadBaseline(): LockBaseline {
  return JSON.parse(readFileSync(LOCK_PATH, "utf8")) as LockBaseline;
}

function loadManifestSelections(): {
  selected: string[];
  excluded: string[];
} {
  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8")) as {
    operations: string[];
    deferredOperations: Array<{ operation: string }>;
    unsupportedOperations: Array<{ operation: string }>;
    sdkExcludedOperations: Array<{ operation: string }>;
  };
  const subset = JSON.parse(readFileSync(SUBSET_PATH, "utf8")) as {
    "x-open-compute-operation-inventory": Array<{
      id: string;
      operationSha256: string;
    }>;
  };
  return {
    selected: [
      ...manifest.operations,
      ...manifest.deferredOperations.map((entry) => entry.operation),
      ...manifest.unsupportedOperations.map((entry) => entry.operation),
    ],
    excluded: manifest.sdkExcludedOperations.map((entry) => entry.operation),
  };
}

async function npmIdentity(name: string): Promise<NpmIdentity> {
  const response = await fetch(`https://registry.npmjs.org/${name}/latest`);
  if (!response.ok)
    throw new Error(
      `npm registry lookup for ${name} failed: ${response.status}`,
    );
  const metadata = (await response.json()) as {
    version: string;
    dist: { shasum: string; integrity: string };
  };
  return {
    version: metadata.version,
    npmShasum: metadata.dist.shasum,
    npmIntegrity: metadata.dist.integrity,
  };
}

function headRevision(): string {
  const result = spawnSync(
    "git",
    ["ls-remote", "https://github.com/cloudflare/api-schemas", "HEAD"],
    { encoding: "utf8" },
  );
  if (result.status !== 0 || !result.stdout)
    throw new Error("cannot resolve the api-schemas HEAD revision");
  return result.stdout.trim().split(/\s+/)[0] ?? "";
}

async function downloadSchema(revision: string): Promise<Buffer> {
  const response = await fetch(
    `https://raw.githubusercontent.com/cloudflare/api-schemas/${revision}/openapi.json`,
  );
  if (!response.ok)
    throw new Error(`api-schemas snapshot download failed: ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
}

async function extractCandidatePackage(
  name: string,
  identity: NpmIdentity,
  target: string,
): Promise<ExtractedPackage> {
  const tarballResponse = await fetch(
    `https://registry.npmjs.org/${name}/-/${name}-${identity.version}.tgz`,
  );
  if (!tarballResponse.ok)
    throw new Error(
      `${name} tarball download failed: ${tarballResponse.status}`,
    );
  const tarball = Buffer.from(await tarballResponse.arrayBuffer());
  const shasum = createHash("sha1").update(tarball).digest("hex");
  const integrity = `sha512-${createHash("sha512").update(tarball).digest("base64")}`;
  if (shasum !== identity.npmShasum || integrity !== identity.npmIntegrity)
    throw new Error(
      `${name} candidate tarball does not match registry identity`,
    );
  const extractRoot = join(REVIEW_ROOT, target);
  rmSync(extractRoot, { recursive: true, force: true });
  mkdirSync(extractRoot, { recursive: true });
  const tarballPath = join(extractRoot, "package.tgz");
  writeFileSync(tarballPath, tarball);
  const extract = spawnSync("tar", ["-xzf", tarballPath, "-C", extractRoot], {
    encoding: "utf8",
  });
  if (extract.status !== 0)
    throw new Error(`${name} tarball extraction failed: ${extract.stderr}`);
  const root = join(extractRoot, "package");
  return {
    root,
    packageSha256: sha256Hex(tarball),
    packageJsonSha256: sha256Hex(readFileSync(join(root, "package.json"))),
  };
}

async function candidateSdkRoutesFor(
  identity: NpmIdentity,
): Promise<Map<string, string[]>> {
  const baseline = loadBaseline();
  const root =
    identity.version === baseline.cloudflareSdk.version
      ? resolve(REPO_ROOT, "packages/sdk/node_modules/cloudflare")
      : (await extractCandidatePackage("cloudflare", identity, "candidate-sdk"))
          .root;
  const scan = await scanOfficialPackage(root);
  const routes = new Map<string, string[]>();
  for (const method of scan.methods.values()) {
    const route = `${method.httpMethod} ${method.pathTemplate}`;
    routes.set(route, [
      ...(routes.get(route) ?? []),
      `${method.module}::${method.className}::${method.method}`,
    ]);
  }
  return routes;
}

function cfEvidence(root: string): CfEvidence {
  const source = JSON.parse(readFileSync(CAPABILITY_SOURCE_PATH, "utf8")) as {
    cf: {
      supportedFields: string[];
      supportedBindings: string[];
      supportedCommands: string[];
    };
  };
  const packageBytes = readFileSync(join(root, "package.json"));
  const cliBytes = readFileSync(join(root, "dist/index.mjs"));
  let schema: { properties?: Record<string, unknown> } | undefined;
  let configBytes: Buffer = Buffer.alloc(0);
  const unknownDifferences: string[] = [];
  try {
    schema = officialCfSchema(root);
    configBytes = Buffer.from(
      prettierJson(`${JSON.stringify(schema, null, 2)}\n`),
    );
  } catch {
    unknownDifferences.push(
      "candidate cf/config dependencies are unavailable; install the exact candidate and qualify the P20 workflow before applying it",
    );
  }
  const cli = readdirSync(join(root, "dist"))
    .filter((name) => name.endsWith(".mjs"))
    .map((name) => readFileSync(join(root, "dist", name), "utf8"))
    .join("\n");
  const missingCommands: string[] = [];
  if (schema !== undefined) {
    for (const command of source.cf.supportedCommands) {
      const result = spawnSync(
        "node",
        [join(root, "bin/cf"), ...command.split(" "), "--help"],
        {
          encoding: "utf8",
          timeout: 10_000,
          env: {
            ...process.env,
            CF_SEND_TELEMETRY: "false",
            DO_NOT_TRACK: "1",
          },
        },
      );
      if (result.status !== 0 || !result.stdout.includes(`cf ${command}`))
        missingCommands.push(command);
    }
  }
  return {
    packageSha256: "",
    packageJsonSha256: sha256Hex(packageBytes),
    configSchemaSha256: sha256Hex(configBytes),
    cliSha256: sha256Hex(cliBytes),
    missingConfigFields:
      schema === undefined
        ? []
        : source.cf.supportedFields.filter(
            (field) => schema.properties?.[field] === undefined,
          ),
    missingBindings: source.cf.supportedBindings.filter(
      (binding) => !cli.includes(binding),
    ),
    missingCommands,
    unknownDifferences,
  };
}

function summarize(report: UpstreamReport): string {
  const lines = [
    `classification: ${report.classification}`,
    `baseline openapi ${report.baseline.openapiRevision.slice(0, 12)} sdk ${report.baseline.cloudflareSdkVersion} cf ${report.baseline.cfVersion}`,
    `candidate openapi ${report.candidate.openapiRevision?.slice(0, 12) ?? "-"} sdk ${report.candidate.cloudflareSdk?.version ?? "-"} cf ${report.candidate.cf?.version ?? "-"}`,
    `selected operations: ${report.selectedOperations}`,
    `changed operations (${report.changedOperations.length}): ${report.changedOperations.slice(0, 20).join(", ")}`,
    `renamed operations (${report.renamedOperations.length})`,
    `removed operations (${report.removedOperations.length})`,
    `unmapped operations (${report.unmappedOperations.length})`,
    ...report.reasons.map((reason) => `reason: ${reason}`),
    ...report.waitingOn.map((owner) => `waiting on: ${owner}`),
  ];
  return lines.join("\n");
}

async function main(): Promise<void> {
  mkdirSync(REVIEW_ROOT, { recursive: true });
  const baseline = loadBaseline();
  const selections = loadManifestSelections();
  const subset = JSON.parse(readFileSync(SUBSET_PATH, "utf8")) as {
    "x-open-compute-operation-inventory": Array<{
      id: string;
      operationSha256: string;
    }>;
  };
  const revision = headRevision();
  const candidateSchemaBytes = await downloadSchema(revision);
  const [cloudflare, cf] = await Promise.all([
    npmIdentity("cloudflare"),
    npmIdentity("cf"),
  ]);
  const sdkRoutes = await candidateSdkRoutesFor(cloudflare);
  const baselineCf = baseline.cf;
  const cfPackage =
    cf.version === baselineCf.version
      ? {
          root: resolve(REPO_ROOT, "node_modules/cf"),
          packageSha256: baselineCf.packageSha256,
          packageJsonSha256: sha256Hex(
            readFileSync(resolve(REPO_ROOT, "node_modules/cf/package.json")),
          ),
        }
      : await extractCandidatePackage("cf", cf, "candidate-cf");
  const candidateCfEvidence = cfEvidence(cfPackage.root);
  candidateCfEvidence.packageSha256 = cfPackage.packageSha256;
  const manifest = JSON.parse(readFileSync(MANIFEST_PATH, "utf8"));
  const surface = JSON.parse(
    readFileSync(join(REPO_ROOT, "packages/sdk/surface.json"), "utf8"),
  ) as { operations: Array<{ operation: string }> };
  const report = classify({
    baseline: {
      openapiRevision: baseline.revision,
      openapiSha256: baseline.sha256,
      cloudflareSdkVersion: baseline.cloudflareSdk.version,
      cfVersion: baseline.cf.version,
    },
    candidateSchema: { revision, bytes: candidateSchemaBytes },
    candidateCloudflare: cloudflare,
    candidateCf: cf,
    candidateCfEvidence,
    candidateSdkRoutes: sdkRoutes,
    manifest,
    sdkExcludedOperations: selections.excluded,
    baselineMappedOperations: surface.operations.map(
      (operation) => operation.operation,
    ),
    baselineInventory: subset["x-open-compute-operation-inventory"],
    selectedOperations: selections.selected,
  });
  writeFileSync(
    join(REVIEW_ROOT, "upstream-review.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  const summary = summarize(report);
  writeFileSync(join(REVIEW_ROOT, "summary.txt"), `${summary}\n`);
  console.log(summary);
}

if (
  process.argv[1] !== undefined &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  await main();
