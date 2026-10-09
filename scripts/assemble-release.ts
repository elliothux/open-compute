import { createHash } from "node:crypto";
import { lstat, open, readdir, readFile } from "node:fs/promises";
import { basename, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { releaseTestReport } from "./release-test-report.ts";
import { command, repository, sha256 } from "./workerd-archive.ts";

const CLOUDFLARE_SDK_LOCK_PATH = `${repository}openapi/upstream/cloudflare-openapi.lock.json`;

export const releaseTargets = [
  "darwin-arm64",
  "linux-arm64",
  "linux-x64",
] as const;

export interface ReleaseIdentity {
  /** Workspace version without the tag's `v` prefix. */
  version: string;
  /** Exact Git commit embedded by every packaged executable. */
  revision: string;
  /** Formally pinned upstream workerd release. */
  workerd: string;
  /** SHA-256 of the authoritative multi-platform workerd lock. */
  workerdLockSha256: string;
}

interface PackageReport extends ReleaseIdentity {
  schemaVersion: number;
  destination: string;
  target: string;
  bytes: number;
  sha256: string;
}

export interface SdkPackageReport {
  /** Report schema consumed by release assembly and publication. */
  schemaVersion: 1;
  /** Published npm package name; must be the one scoped SDK package. */
  package: string;
  /** SDK package version; must equal the workspace release version. */
  packageVersion: string;
  /** Basename of the verified npm tarball. */
  tarball: string;
  /** npm shasum (SHA-1, hex) of the packed tarball. */
  tarballShasum: string;
  /** npm SRI integrity of the packed tarball. */
  tarballIntegrity: string;
  /** SHA-256 of the generated SDK surface report. */
  surfaceDigest: string;
  /** Pinned official Cloudflare OpenAPI revision. */
  openapiRevision: string;
  /** Pinned official `cloudflare` npm version. */
  cloudflareSdkVersion: string;
  /** Exact files admitted to the npm tarball. */
  files: string[];
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`invalid ${label}`);
  }
  return value as Record<string, unknown>;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || !value) throw new Error(`invalid ${label}`);
  return value;
}

export function stableVersionFromTag(tag: string): string {
  const match = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(
    tag,
  );
  if (!match)
    throw new Error("release tag must be stable SemVer in the form vX.Y.Z");
  return tag.slice(1);
}

export function workspaceVersion(source: string): string {
  const lines = source.split(/\r?\n/);
  const start = lines.findIndex((line) => line === "[workspace.package]");
  if (start < 0)
    throw new Error("Cargo.toml must define one workspace package version");
  const section = lines
    .slice(start + 1)
    .findIndex((line) => line.startsWith("["));
  const body = lines
    .slice(start + 1, section < 0 ? undefined : start + 1 + section)
    .join("\n");
  const matches = [...body.matchAll(/^version\s*=\s*"([^"]+)"\s*$/gm)];
  if (matches.length !== 1)
    throw new Error("Cargo.toml must define one workspace package version");
  return stableVersionFromTag(`v${matches[0]?.[1] ?? ""}`);
}

export async function repositoryReleaseIdentity(): Promise<ReleaseIdentity> {
  const cargo = await readFile(`${repository}Cargo.toml`, "utf8");
  const lockBytes = await readFile(
    `${repository}packages/runtime/workerd.lock.json`,
  );
  const lock = record(
    JSON.parse(lockBytes.toString("utf8")) as unknown,
    "workerd lock",
  );
  return {
    version: workspaceVersion(cargo),
    revision: command("git", ["rev-parse", "--verify", "HEAD"]).trim(),
    workerd: string(lock.release, "workerd release"),
    workerdLockSha256: sha256(lockBytes),
  };
}

function packageReport(value: unknown): PackageReport {
  const raw = record(value, "package report");
  const bytes = raw.bytes;
  if (
    raw.schemaVersion !== 1 ||
    typeof bytes !== "number" ||
    !Number.isSafeInteger(bytes) ||
    bytes <= 0
  ) {
    throw new Error("invalid package report schema");
  }
  const digest = string(raw.sha256, "package digest");
  if (!/^[a-f0-9]{64}$/.test(digest)) throw new Error("invalid package digest");
  return {
    schemaVersion: 1,
    destination: string(raw.destination, "package destination"),
    target: string(raw.target, "package target"),
    version: string(raw.version, "package version"),
    revision: string(raw.revision, "package revision"),
    workerd: string(raw.workerd, "package workerd release"),
    workerdLockSha256: string(
      raw.workerdLockSha256,
      "package workerd lock digest",
    ),
    bytes,
    sha256: digest,
  };
}

/** Validate the single SDK report contract shared by packaging and assembly. */
export function parseSdkPackageReport(value: unknown): SdkPackageReport {
  const raw = record(value, "SDK package report");
  if (raw.schemaVersion !== 1)
    throw new Error("invalid SDK package report schema");
  const tarball = string(raw.tarball, "SDK tarball");
  if (!/^[A-Za-z0-9][A-Za-z0-9._-]*\.tgz$/.test(tarball))
    throw new Error("invalid SDK tarball name");
  const shasum = string(raw.tarballShasum, "SDK tarball shasum");
  if (!/^[a-f0-9]{40}$/.test(shasum))
    throw new Error("invalid SDK tarball shasum");
  const integrity = string(raw.tarballIntegrity, "SDK tarball integrity");
  if (!/^sha512-[A-Za-z0-9+/]+={0,2}$/.test(integrity))
    throw new Error("invalid SDK tarball integrity");
  const surfaceDigest = string(raw.surfaceDigest, "SDK surface digest");
  if (!/^[a-f0-9]{64}$/.test(surfaceDigest))
    throw new Error("invalid SDK surface digest");
  const openapiRevision = string(raw.openapiRevision, "SDK OpenAPI revision");
  if (!/^[0-9a-f]{40}$/.test(openapiRevision))
    throw new Error("invalid SDK OpenAPI revision");
  if (
    !Array.isArray(raw.files) ||
    raw.files.length === 0 ||
    raw.files.some(
      (file) =>
        typeof file !== "string" ||
        !file.startsWith("package/") ||
        file.includes(".."),
    ) ||
    new Set(raw.files).size !== raw.files.length
  ) {
    throw new Error("invalid SDK package file inventory");
  }
  return {
    schemaVersion: 1,
    package: string(raw.package, "SDK package name"),
    packageVersion: string(raw.packageVersion, "SDK package version"),
    tarball,
    tarballShasum: shasum,
    tarballIntegrity: integrity,
    surfaceDigest,
    openapiRevision,
    cloudflareSdkVersion: string(
      raw.cloudflareSdkVersion,
      "SDK official Cloudflare version",
    ),
    files: raw.files,
  };
}

function lockedCloudflareSdkVersion(value: unknown): string {
  const lock = record(value, "Cloudflare upstream lock");
  const sdk = record(lock.cloudflareSdk, "Cloudflare upstream lock SDK pin");
  return string(sdk.version, "Cloudflare upstream lock SDK version");
}

async function writeNew(path: string, contents: string): Promise<void> {
  const file = await open(path, "wx", 0o444);
  try {
    await file.writeFile(contents);
    await file.sync();
  } finally {
    await file.close();
  }
}

export async function assembleRelease(
  directory: string,
  tag: string,
  identity: ReleaseIdentity,
  sdk: unknown,
  evidence: string,
): Promise<void> {
  if (!isAbsolute(directory) || resolve(directory) !== directory) {
    throw new Error(
      "release asset directory must be an absolute normalized path",
    );
  }
  const metadata = await lstat(directory);
  if (!metadata.isDirectory())
    throw new Error("release asset path must be a directory");
  const version = stableVersionFromTag(tag);
  if (version !== identity.version)
    throw new Error("release tag does not match the workspace version");
  const checkedSdk = parseSdkPackageReport(sdk);
  if (checkedSdk.package !== "@open-compute/sdk")
    throw new Error("release manifest requires the @open-compute/sdk package");
  if (checkedSdk.packageVersion !== identity.version)
    throw new Error("SDK package version does not match the release version");
  const lockBytes = await readFile(CLOUDFLARE_SDK_LOCK_PATH, "utf8");
  if (
    checkedSdk.cloudflareSdkVersion !==
    lockedCloudflareSdkVersion(JSON.parse(lockBytes))
  )
    throw new Error("SDK official Cloudflare version does not match the lock");

  const expected = new Set(
    releaseTargets.flatMap((target) => [
      `ocd-${tag}-${target}`,
      `release-report-${target}.json`,
    ]),
  );
  const names = await readdir(directory);
  if (
    names.length !== expected.size ||
    names.some((name) => !expected.has(name))
  ) {
    throw new Error(
      "release input directory does not contain the exact three binaries and reports",
    );
  }

  const artifacts = [];
  for (const target of releaseTargets) {
    const filename = `ocd-${tag}-${target}`;
    const path = `${directory}/${filename}`;
    const reportPath = `${directory}/release-report-${target}.json`;
    const binaryMetadata = await lstat(path);
    if (!binaryMetadata.isFile())
      throw new Error(`${filename} is not a regular file`);
    const reportMetadata = await lstat(reportPath);
    if (!reportMetadata.isFile())
      throw new Error(`${target} package report is not a regular file`);
    const report = packageReport(
      JSON.parse(await readFile(reportPath, "utf8")) as unknown,
    );
    const bytes = await readFile(path);
    if (
      report.target !== target ||
      basename(report.destination) !== filename ||
      report.version !== identity.version ||
      report.revision !== identity.revision ||
      report.workerd !== identity.workerd ||
      report.workerdLockSha256 !== identity.workerdLockSha256 ||
      report.bytes !== binaryMetadata.size ||
      report.bytes !== bytes.length ||
      report.sha256 !== sha256(bytes)
    ) {
      throw new Error(
        `${target} package report does not match the immutable release inputs`,
      );
    }
    const [os, arch] = target.split("-") as [string, string];
    artifacts.push({
      target,
      os,
      arch,
      filename,
      bytes: report.bytes,
      sha256: report.sha256,
    });
  }

  const stressCandidate = artifacts.find(
    (artifact) => artifact.target === "linux-x64",
  );
  if (!stressCandidate) throw new Error("missing stress candidate");
  const qualification = await releaseTestReport(
    evidence,
    identity,
    stressCandidate.sha256,
  );
  const testReports = [
    { filename: "test-report.json", contents: qualification.json },
    { filename: "test-report.html", contents: qualification.html },
  ];
  for (const report of testReports)
    await writeNew(`${directory}/${report.filename}`, report.contents);

  const manifest = `${JSON.stringify(
    {
      schemaVersion: 1,
      tag,
      version: identity.version,
      gitRevision: identity.revision,
      workerdRelease: identity.workerd,
      workerdLockSha256: identity.workerdLockSha256,
      sdk: {
        package: checkedSdk.package,
        packageVersion: checkedSdk.packageVersion,
        tarballShasum: checkedSdk.tarballShasum,
        tarballIntegrity: checkedSdk.tarballIntegrity,
        surfaceDigest: checkedSdk.surfaceDigest,
        openapiRevision: checkedSdk.openapiRevision,
        cloudflareSdkVersion: checkedSdk.cloudflareSdkVersion,
      },
      artifacts,
      testReports: testReports.map(({ filename, contents }) => ({
        filename,
        bytes: Buffer.byteLength(contents),
        sha256: sha256(Buffer.from(contents)),
      })),
    },
    null,
    2,
  )}\n`;
  const manifestPath = `${directory}/release.json`;
  await writeNew(manifestPath, manifest);
  const checksums =
    [
      ...artifacts.map(
        (artifact) => `${artifact.sha256}  ${artifact.filename}`,
      ),
      ...testReports.map(
        ({ filename, contents }) =>
          `${sha256(Buffer.from(contents))}  ${filename}`,
      ),
      `${createHash("sha256").update(manifest).digest("hex")}  release.json`,
    ].join("\n") + "\n";
  await writeNew(`${directory}/SHA256SUMS`, checksums);
}

function argumentsFrom(args: string[]): {
  tag: string;
  directory: string;
  sdkReport: string;
  evidence: string;
} {
  let tag: string | undefined;
  let directory: string | undefined;
  let sdkReport: string | undefined;
  let evidence: string | undefined;
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--tag" && tag === undefined) tag = args[++index];
    else if (argument === "--dir" && directory === undefined)
      directory = args[++index];
    else if (argument === "--sdk-report" && sdkReport === undefined)
      sdkReport = args[++index];
    else if (argument === "--evidence" && evidence === undefined)
      evidence = args[++index];
    else
      throw new Error(
        "usage: --tag vX.Y.Z --dir ABS --sdk-report ABS.json --evidence ABS",
      );
  }
  if (!tag || !directory || !sdkReport || !evidence)
    throw new Error(
      "usage: --tag vX.Y.Z --dir ABS --sdk-report ABS.json --evidence ABS",
    );
  return { tag, directory, sdkReport, evidence };
}

if (
  process.argv[1] &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
  const input = argumentsFrom(process.argv.slice(2));
  await assembleRelease(
    input.directory,
    input.tag,
    await repositoryReleaseIdentity(),
    JSON.parse(await readFile(input.sdkReport, "utf8")) as unknown,
    input.evidence,
  );
}
