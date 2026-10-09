import { randomUUID } from "node:crypto";
import { link, mkdtemp, open, readFile, rm, unlink } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { prepareCaddy } from "./caddy-archive.ts";
import { verifyReleaseExecutable } from "./verify-release-executable.ts";
import {
  absoluteDestination,
  cargoTargetDirectory,
  command,
  prepareWorkerd,
  sha256,
  sourceArguments,
} from "./workerd-archive.ts";

const input = sourceArguments(process.argv.slice(2));
const destination = await absoluteDestination(input.destination);
const buildDirectory = cargoTargetDirectory(process.env.CARGO_TARGET_DIR);
if (command("git", ["status", "--porcelain", "--untracked-files=all"]).trim()) {
  throw new Error("release packaging requires a clean checkout");
}
const revision = command("git", ["rev-parse", "--verify", "HEAD"]).trim();
const work = await mkdtemp(join(tmpdir(), "open-compute-release-"));
let ownsTemporary = false;
const temporary = join(dirname(destination), `.ocd-${randomUUID()}`);
try {
  const pin = await prepareWorkerd(work, input.archive, input.download);
  const caddy =
    process.env.OPEN_COMPUTE_BUILD_CADDY ??
    (input.download
      ? (await prepareCaddy(work, undefined, true)).binary
      : undefined);
  if (!caddy)
    throw new Error("prepare Caddy or explicitly package with --download");
  const target = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "linux-x64": "x86_64-unknown-linux-gnu",
  }[pin.target];
  if (!target) throw new Error("unsupported native Cargo target");
  const buildEnvironment = {
    ...process.env,
    OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE: pin.archive,
    OPEN_COMPUTE_TEST_WORKERD: pin.binary,
    OPEN_COMPUTE_BUILD_CADDY: caddy,
  };
  command("bun", ["run", "build"], buildEnvironment);
  command("bun", ["run", "check:generated"]);
  command(
    "mbx",
    [
      "build",
      "--locked",
      "--release",
      "--timings",
      "--target",
      target,
      "-p",
      "open-compute-service",
      "--bin",
      "ocd",
    ],
    {
      ...buildEnvironment,
      OPEN_COMPUTE_GIT_REVISION: revision,
      // CI may isolate release dependencies from debug/test artifacts. The
      // exact native target below still prevents selecting unrelated output.
      CARGO_TARGET_DIR: buildDirectory,
    },
  );
  const source = join(buildDirectory, target, "release/ocd");
  const bytes = await readFile(source);
  const file = await open(temporary, "wx", 0o500);
  ownsTemporary = true;
  try {
    await file.writeFile(bytes);
    await file.chmod(0o555);
    await file.sync();
  } finally {
    await file.close();
  }
  const version = await verifyReleaseExecutable(temporary, work, revision, pin);
  if (
    command("git", ["status", "--porcelain", "--untracked-files=all"]).trim() ||
    command("git", ["rev-parse", "--verify", "HEAD"]).trim() !== revision
  ) {
    throw new Error("source changed during release packaging");
  }
  const size = bytes.length;
  await link(temporary, destination); // Atomic, same-filesystem, and refuses every existing destination.
  await unlink(temporary);
  const parent = await open(dirname(destination), "r");
  try {
    await parent.sync();
  } finally {
    await parent.close();
  }
  console.log(
    JSON.stringify({
      schemaVersion: 1,
      destination,
      target: pin.target,
      version,
      revision,
      workerd: pin.release,
      workerdLockSha256: pin.lockSha256,
      bytes: size,
      sha256: sha256(bytes),
    }),
  );
} finally {
  if (ownsTemporary) await rm(temporary, { force: true });
  await rm(work, { recursive: true, force: true });
}
