import { createHash } from "node:crypto";
import { lstat, open, readFile, readdir } from "node:fs/promises";
import { basename, isAbsolute, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { command, repository, sha256 } from "./workerd-archive.ts";

export const releaseTargets = [
  "darwin-arm64",
  "darwin-x64",
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
  const match = /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(tag);
  if (!match) throw new Error("release tag must be stable SemVer in the form vX.Y.Z");
  return tag.slice(1);
}

export function workspaceVersion(source: string): string {
  const lines = source.split(/\r?\n/);
  const start = lines.findIndex((line) => line === "[workspace.package]");
  if (start < 0) throw new Error("Cargo.toml must define one workspace package version");
  const section = lines.slice(start + 1).findIndex((line) => line.startsWith("["));
  const body = lines.slice(start + 1, section < 0 ? undefined : start + 1 + section).join("\n");
  const matches = [...body.matchAll(/^version\s*=\s*"([^"]+)"\s*$/gm)];
  if (matches.length !== 1) throw new Error("Cargo.toml must define one workspace package version");
  return stableVersionFromTag(`v${matches[0]?.[1] ?? ""}`);
}

export async function repositoryReleaseIdentity(): Promise<ReleaseIdentity> {
  const cargo = await readFile(`${repository}Cargo.toml`, "utf8");
  const lockBytes = await readFile(`${repository}packages/runtime/workerd.lock.json`);
  const lock = record(JSON.parse(lockBytes.toString("utf8")) as unknown, "workerd lock");
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
  if (raw.schemaVersion !== 1 || typeof bytes !== "number" || !Number.isSafeInteger(bytes) || bytes <= 0) {
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
    workerdLockSha256: string(raw.workerdLockSha256, "package workerd lock digest"),
    bytes,
    sha256: digest,
  };
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
): Promise<void> {
  if (!isAbsolute(directory) || resolve(directory) !== directory) {
    throw new Error("release asset directory must be an absolute normalized path");
  }
  const metadata = await lstat(directory);
  if (!metadata.isDirectory()) throw new Error("release asset path must be a directory");
  const version = stableVersionFromTag(tag);
  if (version !== identity.version) throw new Error("release tag does not match the workspace version");

  const expected = new Set(releaseTargets.flatMap((target) => [
    `ocd-${tag}-${target}`,
    `release-report-${target}.json`,
  ]));
  const names = await readdir(directory);
  if (names.length !== expected.size || names.some((name) => !expected.has(name))) {
    throw new Error("release input directory does not contain the exact four binaries and reports");
  }

  const artifacts = [];
  for (const target of releaseTargets) {
    const filename = `ocd-${tag}-${target}`;
    const path = `${directory}/${filename}`;
    const reportPath = `${directory}/release-report-${target}.json`;
    const binaryMetadata = await lstat(path);
    if (!binaryMetadata.isFile()) throw new Error(`${filename} is not a regular file`);
    const reportMetadata = await lstat(reportPath);
    if (!reportMetadata.isFile()) throw new Error(`${target} package report is not a regular file`);
    const report = packageReport(JSON.parse(await readFile(reportPath, "utf8")) as unknown);
    const bytes = await readFile(path);
    if (report.target !== target
      || basename(report.destination) !== filename
      || report.version !== identity.version
      || report.revision !== identity.revision
      || report.workerd !== identity.workerd
      || report.workerdLockSha256 !== identity.workerdLockSha256
      || report.bytes !== binaryMetadata.size
      || report.bytes !== bytes.length
      || report.sha256 !== sha256(bytes)) {
      throw new Error(`${target} package report does not match the immutable release inputs`);
    }
    const [os, arch] = target.split("-") as [string, string];
    artifacts.push({ target, os, arch, filename, bytes: report.bytes, sha256: report.sha256 });
  }

  const manifest = `${JSON.stringify({
    schemaVersion: 1,
    tag,
    version: identity.version,
    gitRevision: identity.revision,
    workerdRelease: identity.workerd,
    workerdLockSha256: identity.workerdLockSha256,
    artifacts,
  }, null, 2)}\n`;
  const manifestPath = `${directory}/release.json`;
  await writeNew(manifestPath, manifest);
  const checksums = [
    ...artifacts.map((artifact) => `${artifact.sha256}  ${artifact.filename}`),
    `${createHash("sha256").update(manifest).digest("hex")}  release.json`,
  ].join("\n") + "\n";
  await writeNew(`${directory}/SHA256SUMS`, checksums);
}

function argumentsFrom(args: string[]): { tag: string; directory: string } {
  let tag: string | undefined;
  let directory: string | undefined;
  for (let index = 0; index < args.length; index += 1) {
    const argument = args[index];
    if (argument === "--tag" && tag === undefined) tag = args[++index];
    else if (argument === "--dir" && directory === undefined) directory = args[++index];
    else throw new Error("usage: --tag vX.Y.Z --dir ABS");
  }
  if (!tag || !directory) throw new Error("usage: --tag vX.Y.Z --dir ABS");
  return { tag, directory };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const input = argumentsFrom(process.argv.slice(2));
  await assembleRelease(input.directory, input.tag, await repositoryReleaseIdentity());
}
