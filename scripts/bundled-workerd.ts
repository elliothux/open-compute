import { createHash, randomUUID } from "node:crypto";
import { link, lstat, mkdir, open, readFile, unlink } from "node:fs/promises";
import { join } from "node:path";
import { gzipSync } from "node:zlib";
import type { loadPin } from "./workerd-archive.ts";

type Pin = Awaited<ReturnType<typeof loadPin>>;
const maxBinary = 256 * 1024 * 1024;
const maxArchive = 64 * 1024 * 1024;
const digest = (bytes: Uint8Array) =>
  createHash("sha256").update(bytes).digest("hex");

/** One pinned compressor for Cargo, input preparation, and release packaging. */
export function compressWorkerd(binary: Uint8Array): Buffer {
  if (process.versions.bun !== "1.3.14")
    throw new Error("workerd archives require Bun 1.3.14");
  if (binary.length > maxBinary)
    throw new Error("bundled workerd exceeds the size bound");
  const archive = gzipSync(binary, { level: 9 });
  // No platform-dependent gzip OS marker; gzipSync already omits filename and timestamp.
  archive[9] = 255;
  return archive;
}

async function directory(path: string): Promise<void> {
  try {
    await mkdir(path);
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "EEXIST"))
      throw error;
  }
  if (!(await lstat(path)).isDirectory())
    throw new Error("workerd cache path must be a physical directory");
}

/** Verify the checked-in binary and materialize only its exact pinned compressed bytes. */
export async function bundledWorkerdArchive(
  repository: string,
  pin: Pin,
): Promise<string> {
  const source = join(repository, "share", "workerd", pin.target);
  for (const path of [
    join(repository, "share"),
    join(repository, "share/workerd"),
    source,
  ]) {
    if (!(await lstat(path)).isDirectory())
      throw new Error("bundled workerd path must be a physical directory");
  }
  const binaryPath = join(source, "workerd");
  const metadata = await lstat(binaryPath);
  if (
    !metadata.isFile() ||
    metadata.size > maxBinary ||
    (metadata.mode & 0o111) === 0
  ) {
    throw new Error(
      "bundled workerd must be a bounded executable regular file",
    );
  }
  const binary = await readFile(binaryPath);
  if (digest(binary) !== pin.binarySha256) {
    throw new Error(
      "bundled workerd SHA-256 mismatch; hydrate Git LFS files and verify the formal pin",
    );
  }
  let cache = repository;
  for (const part of [
    ".temp",
    "workerd-build",
    pin.target,
    pin.archiveSha256,
  ]) {
    cache = join(cache, part);
    await directory(cache);
  }
  const destination = join(cache, pin.archiveName);
  const verify = async () => {
    const metadata = await lstat(destination);
    if (
      !metadata.isFile() ||
      metadata.size > maxArchive ||
      digest(await readFile(destination)) !== pin.archiveSha256
    ) {
      throw new Error("cached workerd archive does not match the formal pin");
    }
  };
  try {
    await verify();
    return destination;
  } catch (error) {
    if (!(error instanceof Error && "code" in error && error.code === "ENOENT"))
      throw error;
  }
  const archive = compressWorkerd(binary);
  if (archive.length > maxArchive || digest(archive) !== pin.archiveSha256) {
    throw new Error("generated workerd archive does not match the formal pin");
  }
  const temporary = join(cache, `.archive-${randomUUID()}`);
  const file = await open(temporary, "wx", 0o600);
  try {
    try {
      await file.writeFile(archive);
      await file.chmod(0o444);
      await file.sync();
    } finally {
      await file.close();
    }
    try {
      await link(temporary, destination);
    } catch (error) {
      if (!(
        error instanceof Error &&
        "code" in error &&
        error.code === "EEXIST"
      ))
        throw error;
      await verify();
    }
  } finally {
    await unlink(temporary);
  }
  return destination;
}
