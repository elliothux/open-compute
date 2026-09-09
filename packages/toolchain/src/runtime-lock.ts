import { constants } from "node:fs";
import { open } from "node:fs/promises";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const LOCK_PATH = resolve(
  fileURLToPath(new URL("../../runtime/workerd.lock.json", import.meta.url)),
);
const MAX_LOCK_BYTES = 64 * 1024;
const SCHEMA_VERSION = 3;

/** Formal lock fields the toolchain may read. Date/flags stay internal executable identity. */
export interface FormalRuntimeLock {
  readonly effectiveCompatibilityDate: string;
  readonly requiredCompatibilityFlags: readonly string[];
  readonly systemCompatibilityFlags: readonly string[];
}

function isErrno(error: unknown, code: string): boolean {
  return error instanceof Error && "code" in error && error.code === code;
}

function record(value: unknown, label: string): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must be an object`);
  }
  return value as Record<string, unknown>;
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0)
    throw new Error(`invalid ${label}`);
  return value;
}

function gitSha(value: unknown, label: string): string {
  const sha = string(value, label);
  if (sha.length !== 40 || !/^[0-9a-f]+$/.test(sha))
    throw new Error(`invalid ${label}`);
  return sha;
}

function sha256(value: unknown, label: string): string {
  const digest = string(value, label);
  if (!/^[0-9a-f]{64}$/.test(digest)) throw new Error(`invalid ${label}`);
  return digest;
}

function validGregorianDate(year: number, month: number, day: number): boolean {
  if (year < 1970 || month < 1 || month > 12 || day < 1) return false;
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
  const max = days[month - 1];
  return max !== undefined && day <= max;
}

function compatibilityDate(value: unknown): string {
  const date = string(value, "effectiveCompatibilityDate");
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date))
    throw new Error("invalid effectiveCompatibilityDate");
  const year = Number(date.slice(0, 4));
  const month = Number(date.slice(5, 7));
  const day = Number(date.slice(8, 10));
  if (!validGregorianDate(year, month, day)) {
    throw new Error("effectiveCompatibilityDate is not a real calendar date");
  }
  return date;
}

function flags(value: unknown, label: string): string[] {
  if (
    !Array.isArray(value) ||
    !value.every((item): item is string => typeof item === "string")
  ) {
    throw new Error(`invalid ${label}`);
  }
  const seen = new Set<string>();
  for (const flag of value) {
    if (!flag || !/^[A-Za-z0-9_]+$/.test(flag))
      throw new Error("compatibility flag is malformed");
    if (seen.has(flag)) throw new Error("compatibility flag is duplicated");
    seen.add(flag);
  }
  return value;
}

function requireCurrentSchema(lock: Record<string, unknown>): void {
  if (lock.schemaVersion !== SCHEMA_VERSION) {
    throw new Error("unsupported workerd lock schema version");
  }
  string(lock.release, "release");
  gitSha(lock.revision, "revision");
  const source = record(lock.source, "source");
  if (source.repository !== "https://github.com/elliothux/workerd")
    throw new Error("invalid source.repository");
  gitSha(source.upstreamBase, "source.upstreamBase");
  const buildInputs = record(source.buildInputs, "source.buildInputs");
  string(buildInputs.bazel, "source.buildInputs.bazel");
  if (
    buildInputs.target !== "//src/workerd/server:workerd" ||
    buildInputs.mode !== "opt"
  ) {
    throw new Error("invalid source.buildInputs target or mode");
  }
  string(lock.expectedVersionOutput, "expectedVersionOutput");
  const pyodide = record(lock.pyodideBundle, "pyodideBundle");
  const pyodideBundleVersion = string(pyodide.version, "pyodideBundle.version");
  const versionSeparator = pyodideBundleVersion.indexOf("_");
  const pyodideVersion =
    versionSeparator > 0 ? pyodideBundleVersion.slice(0, versionSeparator) : "";
  if (
    !/^\d+\.\d+\.\d+$/.test(pyodideVersion) ||
    pyodide.fileName !== `pyodide_${pyodideBundleVersion}.capnp.bin` ||
    pyodide.archiveName !== `${pyodide.fileName}.gz` ||
    buildInputs.pyodideBundleTarget !==
      `//src/pyodide:pyodide.capnp.bin@rule@${pyodideVersion}`
  ) {
    throw new Error("invalid pyodideBundle identity");
  }
  sha256(pyodide.archiveSha256, "pyodideBundle.archiveSha256");
  sha256(pyodide.bundleSha256, "pyodideBundle.bundleSha256");
  const workersTypes = record(lock.workersTypes, "workersTypes");
  string(workersTypes.version, "workersTypes.version");
  gitSha(workersTypes.gitHead, "workersTypes.gitHead");
  string(workersTypes.packageSha256, "workersTypes.packageSha256");
  string(workersTypes.astSha256, "workersTypes.astSha256");
  const workersSdk = record(lock.workersSdk, "workersSdk");
  gitSha(workersSdk.revision, "workersSdk.revision");
  string(workersSdk.wranglerVersion, "workersSdk.wranglerVersion");
  string(workersSdk.vitePluginVersion, "workersSdk.vitePluginVersion");
  const targets = record(lock.targets, "targets");
  if (Object.keys(targets).length === 0)
    throw new Error("workerd lock must list at least one target");
  if (!Array.isArray(lock.processFlags) || lock.processFlags.length === 0) {
    throw new Error("invalid processFlags");
  }
}

async function readLockBytes(path: string): Promise<string> {
  let file: Awaited<ReturnType<typeof open>>;
  try {
    file = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  } catch (error) {
    if (isErrno(error, "ELOOP") || isErrno(error, "EEXIST")) {
      throw new Error("formal runtime lock cannot be a symbolic link");
    }
    if (isErrno(error, "EISDIR")) {
      throw new Error(
        "formal runtime lock must be a regular file of at most 64 KiB",
      );
    }
    throw error;
  }
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > MAX_LOCK_BYTES) {
      throw new Error(
        "formal runtime lock must be a regular file of at most 64 KiB",
      );
    }
    const bytes = Buffer.alloc(info.size);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(
        bytes,
        offset,
        bytes.length - offset,
        offset,
      );
      if (!bytesRead) break;
      offset += bytesRead;
    }
    const after = await file.stat();
    if (
      offset !== bytes.length ||
      after.size !== info.size ||
      after.mtimeMs !== info.mtimeMs
    ) {
      throw new Error(
        "formal runtime lock is truncated or changed during read",
      );
    }
    try {
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } catch {
      throw new Error("formal runtime lock must be valid UTF-8");
    }
  } finally {
    await file.close();
  }
}

function parseFormalRuntimeLock(content: string): FormalRuntimeLock {
  let value: unknown;
  try {
    value = JSON.parse(content);
  } catch {
    throw new Error("formal runtime lock must be valid JSON");
  }
  const lock = record(value, "formal runtime lock");
  requireCurrentSchema(lock);
  const requiredCompatibilityFlags = flags(
    lock.requiredCompatibilityFlags,
    "requiredCompatibilityFlags",
  );
  const systemCompatibilityFlags = flags(
    lock.systemCompatibilityFlags,
    "systemCompatibilityFlags",
  );
  const requiredSet = new Set(requiredCompatibilityFlags);
  if (systemCompatibilityFlags.some((flag) => requiredSet.has(flag))) {
    throw new Error("required and system compatibility flags must be disjoint");
  }
  return {
    effectiveCompatibilityDate: compatibilityDate(
      lock.effectiveCompatibilityDate,
    ),
    requiredCompatibilityFlags,
    systemCompatibilityFlags,
  };
}

/** Load a lock from an explicit path. Production always uses the immutable formal lock path. */
export async function loadFormalRuntimeLockAt(
  path: string,
): Promise<FormalRuntimeLock> {
  return parseFormalRuntimeLock(await readLockBytes(path));
}

/** Load the verified formal runtime lock with a bounded read. No date constant is copied. */
export async function loadFormalRuntimeLock(): Promise<FormalRuntimeLock> {
  return loadFormalRuntimeLockAt(LOCK_PATH);
}
