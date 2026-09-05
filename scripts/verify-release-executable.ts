import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { isAbsolute, join } from "node:path";
import { tmpdir } from "node:os";
import { pathToFileURL } from "node:url";
import { command, loadPin } from "./workerd-archive.ts";

/** Check the embedded CLI/release contract without packaging or initializing platform data. */
export async function verifyReleaseExecutable(
  binary: string,
  directory: string,
  revision: string,
  pin: { expectedVersion: string; lockSha256: string },
): Promise<string> {
  const config = join(directory, "release-check.toml");
  await writeFile(config, command(binary, ["config", "init", "--data-dir", join(directory, "data")]),
    { flag: "wx", mode: 0o600 });
  const raw: unknown = JSON.parse(command(binary, ["--config", config, "capabilities", "--json"]));
  if (typeof raw !== "object" || raw === null || !("release" in raw)
      || typeof raw.release !== "object" || raw.release === null) throw new Error("invalid release capabilities");
  const release = raw.release;
  if (!("git_revision" in release) || release.git_revision !== revision
    || !("workerd_version" in release) || release.workerd_version !== pin.expectedVersion
    || !("workerd_lock_sha256" in release) || release.workerd_lock_sha256 !== pin.lockSha256
    || !("platform_version" in release) || typeof release.platform_version !== "string"
    || command(binary, ["--version"]).trim() !== `ocd ${release.platform_version}`) {
    throw new Error("single executable release identity does not match the build inputs");
  }
  command(binary, ["licenses"]);
  command(binary, ["docs", "install-and-first-start"]);
  return release.platform_version;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [binary, revision] = process.argv.slice(2);
  if (!binary || !isAbsolute(binary) || !revision || process.argv.length !== 4) {
    throw new Error("usage: verify-release-executable.ts ABS_BINARY EXPECTED_REVISION");
  }
  const directory = await mkdtemp(join(tmpdir(), "oc-release-cli-check-"));
  try { await verifyReleaseExecutable(binary, directory, revision, await loadPin()); }
  finally { await rm(directory, { recursive: true, force: true }); }
}
