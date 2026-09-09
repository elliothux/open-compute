import { lstat, readFile } from "node:fs/promises";
import { join } from "node:path";
import { gunzipSync } from "node:zlib";
import { sha256, type loadPyodidePin } from "./workerd-archive.ts";

type Pin = Awaited<ReturnType<typeof loadPyodidePin>>;
const maxArchive = 16 * 1024 * 1024;
const maxBundle = 32 * 1024 * 1024;

/** Verify the platform-independent Pyodide gzip checked into Git LFS. */
export async function verifyBundledPyodide(
  repository: string,
  pin: Pin,
): Promise<void> {
  const directory = join(repository, "share/pyodide");
  if (!(await lstat(directory)).isDirectory())
    throw new Error("bundled Pyodide path must be a physical directory");
  const path = join(directory, pin.archiveName);
  const metadata = await lstat(path);
  if (!metadata.isFile() || metadata.size > maxArchive)
    throw new Error("bundled Pyodide archive must be a bounded regular file");
  const archive = await readFile(path);
  if (sha256(archive) !== pin.archiveSha256)
    throw new Error(
      "bundled Pyodide archive SHA-256 mismatch; hydrate Git LFS files and verify the formal pin",
    );
  const bundle = gunzipSync(archive, { maxOutputLength: maxBundle });
  if (sha256(bundle) !== pin.bundleSha256)
    throw new Error("bundled Pyodide payload does not match the formal pin");
}
