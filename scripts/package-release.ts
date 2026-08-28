import { link, mkdtemp, open, readFile, rm, unlink } from "node:fs/promises";
import { randomUUID } from "node:crypto";
import { dirname, join } from "node:path";
import { tmpdir } from "node:os";
import { absoluteDestination, command, prepareWorkerd, repository, sha256, sourceArguments } from "./workerd-archive.ts";

const input = sourceArguments(process.argv.slice(2));
const destination = await absoluteDestination(input.destination);
if (command("git", ["status", "--porcelain", "--untracked-files=all"]).trim()) {
  throw new Error("release packaging requires a clean checkout");
}
const revision = command("git", ["rev-parse", "--verify", "HEAD"]).trim();
const work = await mkdtemp(join(tmpdir(), "open-compute-release-"));
let ownsTemporary = false;
const temporary = join(dirname(destination), `.platformd-${randomUUID()}`);
try {
  const pin = await prepareWorkerd(work, input.archive, input.download);
  const target = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin",
    "linux-arm64": "aarch64-unknown-linux-gnu", "linux-x64": "x86_64-unknown-linux-gnu" }[pin.target];
  if (!target) throw new Error("unsupported native Cargo target");
  command("bun", ["run", "build"]);
  command("bun", ["run", "check:generated"]);
  command("cargo", ["build", "--locked", "--release", "--target", target, "-p", "open-compute-service", "--bin", "platformd"], {
    ...process.env,
    OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE: pin.archive,
    OPEN_COMPUTE_GIT_REVISION: revision,
    // One native target and one fresh build directory contract; never select arbitrary stale output.
    CARGO_TARGET_DIR: join(repository, "target"),
  });
  const source = join(repository, "target", target, "release/platformd");
  const bytes = await readFile(source);
  const file = await open(temporary, "wx", 0o500);
  ownsTemporary = true;
  try { await file.writeFile(bytes); await file.chmod(0o555); await file.sync(); }
  finally { await file.close(); }
  const raw: unknown = JSON.parse(command(temporary, ["capabilities", "--json"]));
  if (typeof raw !== "object" || raw === null || !("release" in raw)
      || typeof raw.release !== "object" || raw.release === null) throw new Error("invalid release capabilities");
  const release = raw.release;
  if (!("git_revision" in release) || release.git_revision !== revision
    || !("workerd_version" in release) || release.workerd_version !== pin.expectedVersion
    || !("workerd_lock_sha256" in release) || release.workerd_lock_sha256 !== pin.lockSha256
    || !("platform_version" in release) || typeof release.platform_version !== "string"
    || command(temporary, ["--version"]).trim() !== `platformd ${release.platform_version}`) {
    throw new Error("single executable release identity does not match the build inputs");
  }
  command(temporary, ["licenses"]);
  command(temporary, ["docs", "install-and-first-start"]);
  if (command("git", ["status", "--porcelain", "--untracked-files=all"]).trim()
      || command("git", ["rev-parse", "--verify", "HEAD"]).trim() !== revision) {
    throw new Error("source changed during release packaging");
  }
  const size = bytes.length;
  await link(temporary, destination); // Atomic, same-filesystem, and refuses every existing destination.
  await unlink(temporary);
  const parent = await open(dirname(destination), "r");
  try { await parent.sync(); } finally { await parent.close(); }
  console.log(JSON.stringify({ destination, target: pin.target, revision, workerd: pin.release, bytes: size, sha256: sha256(bytes) }));
} finally {
  if (ownsTemporary) await rm(temporary, { force: true });
  await rm(work, { recursive: true, force: true });
}
