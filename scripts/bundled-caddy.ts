import { lstat, readFile } from "node:fs/promises";
import { join } from "node:path";
import { repository, sha256 } from "./workerd-archive.ts";

const targets = [
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64",
  "linux-x64",
] as const;

/** Verify every formally pinned Caddy binary checked into Git LFS. */
export async function verifyBundledCaddy(): Promise<string> {
  const lock = JSON.parse(
    await readFile(join(repository, "packages/caddy/caddy.lock.json"), "utf8"),
  ) as unknown;
  if (typeof lock !== "object" || lock === null || Array.isArray(lock))
    throw new Error("formal Caddy pin is invalid");
  const value = lock as Record<string, unknown>;
  if (
    value.schemaVersion !== 1 ||
    typeof value.release !== "string" ||
    typeof value.expectedVersionOutput !== "string" ||
    typeof value.targets !== "object" ||
    value.targets === null ||
    Array.isArray(value.targets)
  )
    throw new Error("formal Caddy pin is invalid");
  const pins = value.targets as Record<string, unknown>;
  if (Object.keys(pins).sort().join() !== [...targets].sort().join())
    throw new Error("formal Caddy target set is invalid");
  for (const target of targets) {
    const pin = pins[target];
    if (typeof pin !== "object" || pin === null || Array.isArray(pin))
      throw new Error("formal Caddy target pin is invalid");
    const expected = (pin as Record<string, unknown>).binarySha256;
    const path = join(repository, "share/caddy", target, "caddy");
    const metadata = await lstat(path);
    if (
      typeof expected !== "string" ||
      !/^[a-f0-9]{64}$/.test(expected) ||
      !metadata.isFile() ||
      metadata.size > 128 * 1024 * 1024 ||
      (metadata.mode & 0o111) === 0 ||
      sha256(await readFile(path)) !== expected
    )
      throw new Error(
        `bundled Caddy mismatch for ${target}; hydrate Git LFS files and verify the formal pin`,
      );
  }
  return value.release;
}
