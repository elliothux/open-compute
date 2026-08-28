import { createHash } from "node:crypto";
import { lstat, open, readFile, realpath } from "node:fs/promises";
import { dirname, isAbsolute, join, parse, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { gunzipSync } from "node:zlib";

export const repository = fileURLToPath(new URL("../", import.meta.url));
const maxArchive = 64 * 1024 * 1024;
const maxBinary = 256 * 1024 * 1024;

export function sha256(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

export function hostTarget(): string {
  const os = process.platform === "darwin" ? "darwin" : process.platform === "linux" ? "linux" : "";
  const arch = process.arch === "arm64" ? "arm64" : process.arch === "x64" ? "x64" : "";
  if (!os || !arch) throw new Error("unsupported release host");
  return `${os}-${arch}`;
}

function record(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("invalid workerd pin");
  return value as Record<string, unknown>;
}

function string(value: unknown): string {
  if (typeof value !== "string" || !value) throw new Error("invalid workerd pin field");
  return value;
}

export async function loadPin() {
  const bytes = await readFile(join(repository, "runtime/workerd.lock.json"));
  const lock = record(JSON.parse(bytes.toString("utf8")) as unknown);
  const target = hostTarget();
  const entry = record(record(lock.targets)[target]);
  const release = string(lock.release);
  const archiveName = string(entry.archiveName);
  const archiveUrl = string(entry.archiveUrl);
  const archiveSha256 = string(entry.archiveSha256);
  const binarySha256 = string(entry.binarySha256);
  const expectedVersion = string(lock.expectedVersionOutput);
  const expectedName = `workerd-${target.replace("-x64", "-64")}.gz`;
  if (lock.schemaVersion !== 1 || !/^v1\.\d{8}\.\d+$/.test(release)
    || archiveName !== expectedName
    || archiveUrl !== `https://github.com/cloudflare/workerd/releases/download/${release}/${archiveName}`
    || !/^[a-f0-9]{64}$/.test(archiveSha256) || !/^[a-f0-9]{64}$/.test(binarySha256)) {
    throw new Error("formal workerd pin does not match the official target/release contract");
  }
  return { target, release, archiveName, archiveUrl, archiveSha256, binarySha256, expectedVersion, lockSha256: sha256(bytes) };
}

export async function absoluteDestination(path: string): Promise<string> {
  if (!isAbsolute(path) || /[\r\n]/.test(path) || path.split("/").includes("..") || parse(path).root === path) {
    throw new Error("destination must be an explicit absolute non-root path without '..'");
  }
  let parent = dirname(path);
  const ancestors: string[] = [];
  while (parent !== parse(parent).root) { ancestors.push(parent); parent = dirname(parent); }
  for (const ancestor of ancestors.reverse()) {
    const metadata = await lstat(ancestor);
    const systemAlias = process.platform === "darwin" && ["/var", "/tmp"].includes(ancestor)
      && await realpath(ancestor) === `/private${ancestor}`;
    if (!metadata.isDirectory() && !systemAlias) throw new Error("destination must not have symlink ancestors");
  }
  try { await lstat(path); }
  catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") return resolve(path);
    throw error;
  }
  throw new Error("destination already exists; overwrite is forbidden");
}

export function command(program: string, args: string[], environment = process.env): string {
  const result = spawnSync(program, args, {
    cwd: repository, env: environment, encoding: "utf8", maxBuffer: 4 * 1024 * 1024,
  });
  if (result.error || result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    throw new Error(`${program} failed`);
  }
  return result.stdout;
}

export async function prepareWorkerd(directory: string, archivePath: string | undefined, download: boolean) {
  if (download === (archivePath !== undefined)) throw new Error("choose exactly one of --archive ABS or --download");
  const pin = await loadPin();
  let archive: Buffer;
  if (archivePath !== undefined) {
    if (!isAbsolute(archivePath) || !(await lstat(archivePath)).isFile()) throw new Error("archive must be an absolute regular file");
    if ((await lstat(archivePath)).size > maxArchive) throw new Error("archive exceeds the size bound");
    archive = await readFile(archivePath);
  } else {
    // The only runtime download path is this explicitly requested build-time operation.
    const response = await fetch(pin.archiveUrl, { signal: AbortSignal.timeout(120_000) });
    if (!response.ok || !response.body) throw new Error("official workerd archive download failed");
    const chunks: Uint8Array[] = [];
    let total = 0;
    for await (const chunk of response.body) {
      total += chunk.byteLength;
      if (total > maxArchive) throw new Error("download exceeds the archive size bound");
      chunks.push(chunk);
    }
    archive = Buffer.concat(chunks);
  }
  if (archive.length > maxArchive || sha256(archive) !== pin.archiveSha256) throw new Error("archive SHA-256 does not match the formal pin");
  const binary = gunzipSync(archive, { maxOutputLength: maxBinary });
  if (sha256(binary) !== pin.binarySha256) throw new Error("binary SHA-256 does not match the formal pin");
  const outputArchive = join(directory, pin.archiveName);
  const outputBinary = join(directory, "workerd");
  for (const [path, bytes, mode] of [[outputArchive, archive, 0o400], [outputBinary, binary, 0o500]] as const) {
    const file = await open(path, "wx", 0o600);
    try { await file.writeFile(bytes); await file.chmod(mode); await file.sync(); }
    finally { await file.close(); }
  }
  const version = spawnSync(outputBinary, ["--version"], {
    encoding: "utf8", timeout: 20_000, killSignal: "SIGKILL", maxBuffer: 4096,
  });
  if (version.error || version.status !== 0 || version.stdout.trim() !== pin.expectedVersion) {
    throw new Error("verified workerd failed the host version probe");
  }
  return { ...pin, archive: outputArchive, binary: outputBinary };
}

export function sourceArguments(args: string[]): { destination: string; archive: string | undefined; download: boolean } {
  let destination: string | undefined;
  let archive: string | undefined;
  let download = false;
  for (let i = 0; i < args.length; i++) {
    const argument = args[i];
    if (argument === "--download" && !download) download = true;
    else if (argument === "--dest" && destination === undefined) destination = args[++i];
    else if (argument === "--archive" && archive === undefined) archive = args[++i];
    else throw new Error("usage: --dest ABS (--archive ABS | --download)");
  }
  if (!destination || !isAbsolute(destination) || download === (archive !== undefined)) {
    throw new Error("usage: --dest ABS (--archive ABS | --download)");
  }
  return { destination, archive, download };
}
