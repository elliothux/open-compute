import { chmod, mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { isAbsolute, join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import {
  repositoryReleaseIdentity,
  verifyPackageArtifact,
} from "./assemble-release.ts";
import { verifyReleaseExecutable } from "./verify-release-executable.ts";
import { hostTarget, loadPin } from "./workerd-archive.ts";

/** Verify the native package before the CI Broker can execute it. */
export async function verifyReleasePackage(directory: string): Promise<void> {
  if (!isAbsolute(directory) || resolve(directory) !== directory)
    throw new Error("package directory must be an absolute normalized path");
  const identity = await repositoryReleaseIdentity();
  const target = hostTarget();
  await verifyPackageArtifact(directory, target, identity);
  const binary = join(directory, `ocd-v${identity.version}-${target}`);
  await chmod(binary, 0o555);
  const check = await mkdtemp(join(tmpdir(), "oc-package-check-"));
  try {
    if (
      (await verifyReleaseExecutable(
        binary,
        check,
        identity.revision,
        await loadPin(),
      )) !== identity.version
    )
      throw new Error(
        "packaged executable version does not match the workspace",
      );
  } finally {
    await rm(check, { recursive: true, force: true });
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const [directory] = process.argv.slice(2);
  if (!directory || process.argv.length !== 3)
    throw new Error("usage: verify-release-package.ts ABS_DIRECTORY");
  await verifyReleasePackage(directory);
}
