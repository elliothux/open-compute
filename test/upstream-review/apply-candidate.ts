// Apply one frozen upstream-refresh candidate to the current Day1 model.
// Every input is pinned to the digests recorded by the scanner report; this
// script never re-resolves npm `latest` or schema HEAD. It only runs inside
// the `draft-pr` job where humans review the resulting candidate PR.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";

const REPO_ROOT = resolve(import.meta.dirname, "../..");
const REVIEW_ROOT = resolve(REPO_ROOT, ".temp/upstream-review");
const WORK_ROOT = resolve(REVIEW_ROOT, "candidate");
const LOCK_PATH = join(
  REPO_ROOT,
  "openapi/upstream/cloudflare-openapi.lock.json",
);
const CATALOG_PATH = join(REPO_ROOT, "package.json");

const report = JSON.parse(
  readFileSync(join(REVIEW_ROOT, "upstream-review.json"), "utf8"),
);
if (report.schemaVersion !== 1 || report.classification !== "ready")
  throw new Error("the candidate report is not a ready classification");
const candidate = report.candidate;
const schemaRevision = candidate.openapiRevision;
const schemaSha256 = candidate.openapiSha256;
const sdk = candidate.cloudflareSdk;
const wrangler = candidate.wrangler;
if (
  sdk === null ||
  wrangler === null ||
  typeof schemaRevision !== "string" ||
  typeof schemaSha256 !== "string"
)
  throw new Error("candidate report is missing frozen identities");

function sha256(value: Buffer | string): string {
  return createHash("sha256").update(value).digest("hex");
}

function run(command: string, args: string[], cwd = REPO_ROOT): void {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed: ${result.stderr}`);
}

async function download(url: string): Promise<Buffer> {
  const response = await fetch(url);
  if (!response.ok)
    throw new Error(`download failed (${response.status}): ${url}`);
  return Buffer.from(await response.arrayBuffer());
}

async function npmMetadata(
  name: string,
  version: string,
): Promise<Record<string, unknown>> {
  const response = await fetch(`https://registry.npmjs.org/${name}/${version}`);
  if (!response.ok)
    throw new Error(`npm metadata lookup failed for ${name}@${version}`);
  return (await response.json()) as Record<string, unknown>;
}

interface ExtractedTarball {
  root: string;
  tarballSha256: string;
  packageJsonSha256: string;
}

async function extractPackage(
  name: string,
  version: string,
  expectedShasum: string,
  expectedIntegrity: string,
  target: string,
): Promise<ExtractedTarball> {
  const tarball = await download(
    `https://registry.npmjs.org/${name}/-/${name}-${version}.tgz`,
  );
  const shasum = createHash("sha1").update(tarball).digest("hex");
  const integrity = `sha512-${createHash("sha512").update(tarball).digest("base64")}`;
  if (shasum !== expectedShasum || integrity !== expectedIntegrity)
    throw new Error(`${name} candidate tarball does not match frozen identity`);
  const targetRoot = join(WORK_ROOT, target);
  rmSync(targetRoot, { recursive: true, force: true });
  mkdirSync(targetRoot, { recursive: true });
  const tarballPath = join(targetRoot, "package.tgz");
  writeFileSync(tarballPath, tarball);
  run("tar", ["-xzf", "package.tgz"], targetRoot);
  return {
    root: join(targetRoot, "package"),
    tarballSha256: sha256(tarball),
    packageJsonSha256: sha256(
      readFileSync(join(targetRoot, "package/package.json")),
    ),
  };
}

function fileSha(root: string, relative: string): string {
  return sha256(readFileSync(join(root, relative)));
}

rmSync(WORK_ROOT, { recursive: true, force: true });
mkdirSync(WORK_ROOT, { recursive: true });

// 1. Schema snapshot, verified against the frozen report digest.
const schema = await download(
  `https://raw.githubusercontent.com/cloudflare/api-schemas/${schemaRevision}/openapi.json`,
);
if (sha256(schema) !== schemaSha256)
  throw new Error("candidate schema does not match the frozen report digest");
const schemaPath = join(WORK_ROOT, "openapi.json");
writeFileSync(schemaPath, schema);

// 2. Candidate npm packages, verified against the frozen report identities.
const extractedSdk = await extractPackage(
  "cloudflare",
  sdk.version,
  sdk.npmShasum,
  sdk.npmIntegrity,
  "cloudflare",
);
const extractedWrangler = await extractPackage(
  "wrangler",
  wrangler.version,
  wrangler.npmShasum,
  wrangler.npmIntegrity,
  "wrangler",
);

// 3. Preserve the already-verified tag when the SDK did not move; otherwise
// resolve the new immutable tag revision.
const lock = JSON.parse(readFileSync(LOCK_PATH, "utf8"));
let repositoryTagRevision = lock.cloudflareSdk.repositoryTagRevision;
if (lock.cloudflareSdk.version !== sdk.version) {
  const tagResponse = await fetch(
    `https://api.github.com/repos/cloudflare/ts-sdk/git/refs/tags/v${sdk.version}`,
  );
  if (!tagResponse.ok)
    throw new Error(
      `cannot resolve the official SDK tag revision for v${sdk.version}; record it manually`,
    );
  const tagMetadata = (await tagResponse.json()) as {
    object: { sha: string };
  };
  repositoryTagRevision = tagMetadata.object.sha;
}

// 4. Update the dependency catalog and lock identities.
const catalogPackage = JSON.parse(readFileSync(CATALOG_PATH, "utf8"));
catalogPackage.catalog.cloudflare = sdk.version;
catalogPackage.catalog.wrangler = wrangler.version;
writeFileSync(CATALOG_PATH, `${JSON.stringify(catalogPackage, null, 2)}\n`);

lock.revision = schemaRevision;
lock.blobSha = createHash("sha1")
  .update(`blob ${schema.length}\0`)
  .update(schema)
  .digest("hex");
lock.sha256 = schemaSha256;
lock.cloudflareSdk = {
  ...lock.cloudflareSdk,
  version: sdk.version,
  npmIntegrity: sdk.npmIntegrity,
  npmShasum: sdk.npmShasum,
  packageSha256: extractedSdk.tarballSha256,
  packageJsonSha256: extractedSdk.packageJsonSha256,
  npmGitHead: String(
    (await npmMetadata("cloudflare", sdk.version)).gitHead ?? "",
  ),
  repositoryTagRevision,
  indexSha256: fileSha(extractedSdk.root, "index.mjs"),
  workersScriptsResourceSha256: fileSha(
    extractedSdk.root,
    "resources/workers/scripts/scripts.mjs",
  ),
};
lock.wrangler = {
  ...lock.wrangler,
  version: wrangler.version,
  npmIntegrity: wrangler.npmIntegrity,
  npmShasum: wrangler.npmShasum,
  packageSha256: extractedWrangler.tarballSha256,
  packageJsonSha256: extractedWrangler.packageJsonSha256,
  configSchemaSha256: fileSha(extractedWrangler.root, "config-schema.json"),
  cliSha256: fileSha(extractedWrangler.root, "wrangler-dist/cli.js"),
};
writeFileSync(LOCK_PATH, `${JSON.stringify(lock, null, 2)}\n`);

// 5. Regenerate every derived artifact from the candidate inputs.
run("bun", [
  "test/conformance/p6-contract.mjs",
  "generate",
  "--openapi",
  schemaPath,
  "--wrangler-root",
  extractedWrangler.root,
]);
for (const [field, path] of [
  ["subsetSha256", "openapi/cloudflare-v4-subset.json"],
  ["subsetManifestSha256", "openapi/cloudflare-subset-manifest.json"],
  ["extensionSha256", "openapi/open-compute-extension.json"],
  ["observedStandardSha256", "openapi/cloudflare-observed-standard.json"],
  ["capabilitySha256", "openapi/p6-capability.json"],
  ["capabilitySchemaSha256", "openapi/capability-manifest.schema.json"],
] as const) {
  lock[field] = sha256(readFileSync(join(REPO_ROOT, path)));
}
writeFileSync(LOCK_PATH, `${JSON.stringify(lock, null, 2)}\n`);
run("bun", ["packages/sdk/scripts/generate.ts"]);
run("bun", ["test/conformance/inventory.ts", "generate"]);
run("bun", ["install"]);

console.log(
  `applied candidate: openapi ${schemaRevision.slice(0, 12)}, cloudflare ${sdk.version}, wrangler ${wrangler.version}`,
);
