import { lstat, readFile } from "node:fs/promises";
import { join } from "node:path";
import { sha256 } from "./workerd-archive.ts";

interface SourceAsset {
  name: string;
  source: string;
  revision: string;
  license: string;
  size: number;
  sha256: string;
  path: string;
}

interface SourceLock {
  schema_version: number;
  xberg_tesseract_version: string;
  assets: SourceAsset[];
}

/** Verify every offline source consumed by xberg-tesseract's static build. */
export async function verifyBundledTesseractSources(
  repository: string,
): Promise<void> {
  const lockPath = join(
    repository,
    "crates/document-parser/tesseract-source.lock.json",
  );
  const lock = JSON.parse(await readFile(lockPath, "utf8")) as SourceLock;
  if (
    lock.schema_version !== 1 ||
    lock.xberg_tesseract_version !== "1.1.5" ||
    lock.assets.length !== 3
  ) {
    throw new Error("invalid xberg-tesseract source lock");
  }
  for (const asset of lock.assets) {
    if (
      !asset.source.startsWith("https://") ||
      !/^[0-9a-f]{40}$/.test(asset.revision) ||
      !/^[0-9a-f]{64}$/.test(asset.sha256) ||
      !Number.isSafeInteger(asset.size) ||
      asset.size < 1 ||
      !asset.path.startsWith("share/xberg-tesseract-cache/source-artifacts/")
    ) {
      throw new Error("invalid xberg-tesseract source asset contract");
    }
    const path = join(repository, asset.path);
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.size !== asset.size)
      throw new Error(`invalid bundled xberg-tesseract source: ${asset.name}`);
    if (sha256(await readFile(path)) !== asset.sha256)
      throw new Error(
        `bundled xberg-tesseract source SHA-256 mismatch: ${asset.name}; hydrate Git LFS files`,
      );
  }
}
