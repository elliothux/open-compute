import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import {
  assembleRelease,
  releaseTargets,
  stableVersionFromTag,
  workspaceVersion,
} from "../scripts/assemble-release.ts";
import { verifyReleaseExecutable } from "../scripts/verify-release-executable.ts";
import {
  absoluteDestination,
  hostTarget,
  loadPin,
  prepareWorkerd,
  sha256,
  sourceArguments,
} from "../scripts/workerd-archive.ts";

const execFileAsync = promisify(execFile);
const installerPath = fileURLToPath(
  new URL("../scripts/install.sh", import.meta.url),
);
const releaseWorkflowPath = fileURLToPath(
  new URL("../.github/workflows/release.yml", import.meta.url),
);

async function writeTestCommand(directory, name, source) {
  await writeFile(join(directory, name), source, { mode: 0o755 });
}

async function writeTargetCommands(directory) {
  await writeTestCommand(
    directory,
    "id",
    '#!/bin/sh\n[ "$1" = "-u" ] || exit 2\nprintf \'%s\\n\' "${OPEN_COMPUTE_TEST_UID:-1000}"\n',
  );
  await writeTestCommand(
    directory,
    "uname",
    `#!/bin/sh
case "$1" in
  -s) printf '%s\n' "$OPEN_COMPUTE_TEST_OS" ;;
  -m) printf '%s\n' "$OPEN_COMPUTE_TEST_ARCH" ;;
  *) exit 2 ;;
esac
`,
  );
  await writeTestCommand(directory, "sync", "#!/bin/sh\nexit 0\n");
}

test("build inputs default to bundled binaries and require a pinned supported host", async () => {
  assert.equal(
    sourceArguments(["--dest", "/tmp/new", "--archive", "/tmp/pin.gz"]).archive,
    "/tmp/pin.gz",
  );
  assert.equal(
    sourceArguments(["--dest", "/tmp/new", "--download"]).download,
    true,
  );
  assert.equal(sourceArguments(["--dest", "/tmp/new"]).archive, undefined);
  for (const args of [
    [],
    ["--dest", "/tmp/new", "--archive"],
    ["--dest", "relative", "--download"],
    ["--dest", "/tmp/new", "--archive", "/tmp/pin.gz", "--download"],
    ["--dest", "/tmp/new", "--download", "--download"],
  ]) {
    assert.throws(() => sourceArguments(args));
  }
  const pin = await loadPin();
  assert.equal(pin.target, hostTarget());
  assert.match(pin.archiveSha256, /^[a-f0-9]{64}$/);
  if (pin.archiveUrl !== undefined) {
    assert.match(
      pin.archiveUrl,
      /^https:\/\/github\.com\/elliothux\/workerd\/releases\/download\//,
    );
  } else {
    const directory = await mkdtemp(join(tmpdir(), "oc-unpublished-runtime-"));
    try {
      await assert.rejects(
        prepareWorkerd(directory, undefined, true),
        /unpublished.*--archive/,
      );
      assert.deepEqual(await readdir(directory), []);
    } finally {
      await rm(directory, { recursive: true, force: true });
    }
  }
});

test("destinations reject overwrite, traversal, and symlink ancestors", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-input-test-"));
  try {
    const winner = join(root, "winner");
    await writeFile(winner, "keep");
    await assert.rejects(absoluteDestination(winner));
    await assert.rejects(absoluteDestination("relative"));
    await assert.rejects(absoluteDestination("/"));
    await assert.rejects(absoluteDestination(join(root, "sub") + "/../escape"));
    await mkdir(join(root, "directory"));
    await symlink(join(root, "directory"), join(root, "alias"));
    await assert.rejects(absoluteDestination(join(root, "alias/new")));
    assert.equal(
      await absoluteDestination(join(root, "new")),
      join(root, "new"),
    );
    assert.equal(await readFile(winner, "utf8"), "keep");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("wrong archives fail without download, execution, or publication", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-hash-test-"));
  try {
    const archive = join(root, "wrong.gz");
    await writeFile(archive, "not a formal archive");
    await assert.rejects(prepareWorkerd(root, archive, false), /SHA-256/);
    await assert.rejects(prepareWorkerd(root, archive, true), /at most one/);
    await assert.rejects(readFile(join(root, "workerd")));
    assert.equal(
      sha256(Buffer.from("abc")),
      "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release tags are stable SemVer and match the workspace version", () => {
  assert.equal(stableVersionFromTag("v0.1.0"), "0.1.0");
  assert.equal(
    workspaceVersion(
      '[workspace]\n\n[workspace.package]\nversion = "12.3.4"\n\n[dependencies]\n',
    ),
    "12.3.4",
  );
  for (const tag of [
    "0.1.0",
    "v0.1",
    "v01.2.3",
    "v1.2.3-alpha.1",
    "v1.2.3+build",
  ]) {
    assert.throws(() => stableVersionFromTag(tag));
  }
});

test("release qualification runs long checks in parallel without a second Linux workspace Gate", async () => {
  const workflow = await readFile(releaseWorkflowPath, "utf8");
  assert.match(workflow, /  coverage:\n    needs: validate\n/);
  assert.match(workflow, /  integration:\n    needs: validate\n/);
  assert.equal(
    workflow.match(/\.\/test\/gate\.py --workspace --jobs 2/g)?.length,
    1,
  );
  assert.match(workflow, /test-p0-2-egress-linux\.sh p0-2 --jobs 2/);
  assert.doesNotMatch(workflow, /test-p0-2-egress-linux\.sh --workspace/);
  assert.doesNotMatch(workflow, /\n  (?:msrv|lint-test):\n/);
});

test("release assembly requires and describes the exact three native executables", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-assembly-test-"));
  assert.deepEqual(releaseTargets, [
    "darwin-arm64",
    "linux-arm64",
    "linux-x64",
  ]);
  const identity = {
    version: "1.2.3",
    revision: "0123456789abcdef0123456789abcdef01234567",
    workerd: "v1.20260830.1",
    workerdLockSha256: "a".repeat(64),
  };
  try {
    for (const target of releaseTargets) {
      const filename = `ocd-v1.2.3-${target}`;
      const bytes = Buffer.from(`native-${target}`);
      await writeFile(join(root, filename), bytes);
      await writeFile(
        join(root, `release-report-${target}.json`),
        JSON.stringify({
          schemaVersion: 1,
          destination: join(root, filename),
          target,
          ...identity,
          bytes: bytes.length,
          sha256: sha256(bytes),
        }),
      );
    }
    const badReportPath = join(root, "release-report-linux-x64.json");
    const badReport = JSON.parse(await readFile(badReportPath, "utf8"));
    badReport.revision = "f".repeat(40);
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assert.rejects(
      assembleRelease(root, "v1.2.3", identity),
      /does not match/,
    );
    badReport.revision = identity.revision;
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assembleRelease(root, "v1.2.3", identity);
    assert.deepEqual(
      (await readdir(root)).sort(),
      [
        "SHA256SUMS",
        ...releaseTargets.map((target) => `ocd-v1.2.3-${target}`),
        ...releaseTargets.map((target) => `release-report-${target}.json`),
        "release.json",
      ].sort(),
    );
    const manifest = JSON.parse(
      await readFile(join(root, "release.json"), "utf8"),
    );
    assert.equal(manifest.schemaVersion, 1);
    assert.equal(manifest.tag, "v1.2.3");
    assert.equal(manifest.gitRevision, identity.revision);
    assert.deepEqual(
      manifest.artifacts.map((artifact) => artifact.target),
      releaseTargets,
    );
    const checksums = await readFile(join(root, "SHA256SUMS"), "utf8");
    assert.equal(checksums.trim().split("\n").length, 4);
    assert.match(checksums, /  release\.json$/m);
    await assert.rejects(
      assembleRelease(root, "v1.2.3", identity),
      /exact three binaries/,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("release CLI verification supplies a private generated config and rejects identity drift", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-cli-test-"));
  try {
    const binary = join(root, "ocd-fixture");
    await writeFile(
      binary,
      `#!/usr/bin/env node
import { readFileSync, statSync } from "node:fs";
const args = process.argv.slice(2);
if (args[0] === "config" && args[1] === "init") console.log("generated-config");
else if (args[0] === "--config" && args[2] === "capabilities" && args[3] === "--json") {
  if (readFileSync(args[1], "utf8").trim() !== "generated-config"
      || (statSync(args[1]).mode & 0o777) !== 0o600) process.exit(2);
  console.log(JSON.stringify({release: {git_revision:"revision", workerd_version:"workerd pin",
    workerd_lock_sha256:"digest", platform_version:"0.1.0"}}));
} else if (args[0] === "--version") console.log("ocd 0.1.0");
else if (args[0] === "licenses" || args[0] === "docs") console.log("embedded resource");
else process.exit(2);
`,
      { mode: 0o755 },
    );
    const pin = { expectedVersion: "workerd pin", lockSha256: "digest" };
    const good = join(root, "good");
    await mkdir(good);
    assert.equal(
      await verifyReleaseExecutable(binary, good, "revision", pin),
      "0.1.0",
    );
    await assert.rejects(readFile(join(good, "data")), /ENOENT/);
    for (const [name, revision, expected] of [
      ["revision", "different", pin],
      ["pin", "revision", { ...pin, lockSha256: "different" }],
    ]) {
      const directory = join(root, name);
      await mkdir(directory);
      await assert.rejects(
        verifyReleaseExecutable(binary, directory, revision, expected),
        /does not match the build inputs/,
      );
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("installer rejects unwritable destinations before download on every release target", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "oc-install-preflight-test-"));
  const commands = join(root, "commands");
  await mkdir(commands);
  await writeTargetCommands(commands);
  await writeTestCommand(commands, "mkdir", "#!/bin/sh\nexit 73\n");
  await writeTestCommand(
    commands,
    "curl",
    "#!/bin/sh\nprintf 'called\\n' > \"$OPEN_COMPUTE_TEST_CURL_LOG\"\nexit 99\n",
  );
  try {
    for (const [target, os, arch] of [
      ["darwin-arm64", "Darwin", "arm64"],
      ["linux-arm64", "Linux", "aarch64"],
      ["linux-x64", "Linux", "x86_64"],
    ]) {
      await t.test(target, async () => {
        const curlLog = join(root, `${target}-curl.log`);
        await assert.rejects(
          execFileAsync("/bin/sh", [installerPath], {
            env: {
              ...process.env,
              OPEN_COMPUTE_INSTALL_PREFIX: join(root, target),
              OPEN_COMPUTE_RELEASE_TAG: "v1.2.3",
              OPEN_COMPUTE_TEST_ARCH: arch,
              OPEN_COMPUTE_TEST_CURL_LOG: curlLog,
              OPEN_COMPUTE_TEST_OS: os,
              PATH: `${commands}:${process.env.PATH ?? ""}`,
            },
          }),
          (error) => {
            assert.match(error.stderr, /cannot write binary directory:/);
            assert.match(
              error.stderr,
              /system-wide install: sudo sh install\.sh/,
            );
            assert.match(error.stderr, /per-user install: sh install\.sh/);
            assert.doesNotMatch(error.stderr, /fetching/);
            return true;
          },
        );
        await assert.rejects(readFile(curlLog), /ENOENT/);
      });
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("installer preflights the receipt directory separately", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-install-receipt-test-"));
  const commands = join(root, "commands");
  const prefix = join(root, "prefix");
  const curlLog = join(root, "curl.log");
  await mkdir(commands);
  await writeTargetCommands(commands);
  await writeTestCommand(
    commands,
    "mkdir",
    `#!/bin/sh
for argument in "$@"; do
  case "$argument" in
    */share/open-compute) exit 73 ;;
  esac
done
exec /bin/mkdir "$@"
`,
  );
  await writeTestCommand(
    commands,
    "curl",
    "#!/bin/sh\nprintf 'called\\n' > \"$OPEN_COMPUTE_TEST_CURL_LOG\"\nexit 99\n",
  );
  try {
    await assert.rejects(
      execFileAsync("/bin/sh", [installerPath], {
        env: {
          ...process.env,
          OPEN_COMPUTE_INSTALL_PREFIX: prefix,
          OPEN_COMPUTE_RELEASE_TAG: "v1.2.3",
          OPEN_COMPUTE_TEST_ARCH: "x86_64",
          OPEN_COMPUTE_TEST_CURL_LOG: curlLog,
          OPEN_COMPUTE_TEST_OS: "Linux",
          PATH: `${commands}:${process.env.PATH ?? ""}`,
        },
      }),
      (error) => {
        assert.match(error.stderr, /cannot write receipt directory:/);
        assert.doesNotMatch(error.stderr, /fetching/);
        return true;
      },
    );
    await assert.rejects(readFile(curlLog), /ENOENT/);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("root invocation retains the system-wide default prefix", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-install-root-default-test-"));
  const commands = join(root, "commands");
  await mkdir(commands);
  await writeTargetCommands(commands);
  await writeTestCommand(commands, "mkdir", "#!/bin/sh\nexit 73\n");
  try {
    await assert.rejects(
      execFileAsync("/bin/sh", [installerPath], {
        env: {
          ...process.env,
          HOME: root,
          OPEN_COMPUTE_RELEASE_TAG: "v1.2.3",
          OPEN_COMPUTE_TEST_ARCH: "x86_64",
          OPEN_COMPUTE_TEST_OS: "Linux",
          OPEN_COMPUTE_TEST_UID: "0",
          PATH: `${commands}:${process.env.PATH ?? ""}`,
        },
      }),
      (error) => {
        assert.match(
          error.stderr,
          /cannot write binary directory: \/usr\/local\/bin/,
        );
        return true;
      },
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("default non-root install owns one user prefix and configures PATH", async (t) => {
  const root = await mkdtemp(join(tmpdir(), "oc-install-user-test-"));
  const commands = join(root, "commands");
  const releases = join(root, "releases");
  await mkdir(commands);
  await writeTargetCommands(commands);
  try {
    for (const [target, os, arch] of [
      ["darwin-arm64", "Darwin", "arm64"],
      ["linux-arm64", "Linux", "aarch64"],
      ["linux-x64", "Linux", "x86_64"],
    ]) {
      await t.test(target, async () => {
        const release = join(releases, target, "v1.2.3");
        const home = join(root, `${target}-home`);
        const prefix = join(home, ".local");
        const asset = `ocd-v1.2.3-${target}`;
        const binary = Buffer.from(
          '#!/bin/sh\n[ "$1" = "--version" ] || exit 2\nprintf \'ocd 1.2.3\\n\'\n',
        );
        const manifest = Buffer.from(
          `${JSON.stringify({
            version: "1.2.3",
            tag: "v1.2.3",
            artifacts: [{ target }],
          })}\n`,
        );
        await mkdir(release, { recursive: true });
        await writeFile(join(release, asset), binary, { mode: 0o755 });
        await writeFile(join(release, "release.json"), manifest);
        await writeFile(
          join(release, "SHA256SUMS"),
          `${sha256(manifest)}  release.json\n${sha256(binary)}  ${asset}\n`,
        );

        const installEnv = {
          ...process.env,
          HOME: home,
          OPEN_COMPUTE_RELEASE_DOWNLOAD_BASE: `file://${join(releases, target)}`,
          OPEN_COMPUTE_RELEASE_TAG: "v1.2.3",
          OPEN_COMPUTE_TEST_ARCH: arch,
          OPEN_COMPUTE_TEST_OS: os,
          OPEN_COMPUTE_TEST_UID: "1000",
          SHELL: "/bin/zsh",
          PATH: `${commands}:${process.env.PATH ?? ""}`,
        };
        const result = await execFileAsync("/bin/sh", [installerPath], {
          env: installEnv,
        });
        const destination = join(prefix, "bin/ocd");
        const receiptPath = join(
          prefix,
          "share/open-compute/install-receipt.json",
        );
        assert.equal(await readFile(destination, "utf8"), binary.toString());
        const receipt = JSON.parse(await readFile(receiptPath, "utf8"));
        assert.deepEqual(
          { ...receipt, installed_at_ms: 1 },
          {
            schema_version: 1,
            version: "1.2.3",
            sha256: sha256(binary),
            target,
            binary_path: destination,
            method: "install.sh",
            source: `file://${join(releases, target)}/v1.2.3/${asset}`,
            installed_at_ms: 1,
          },
        );
        assert.equal(typeof receipt.installed_at_ms, "number");
        assert(receipt.installed_at_ms > 0);
        assert.match(result.stderr, /installed 1\.2\.3/);
        assert.match(result.stderr, /added .*\.local\/bin to PATH/);
        await execFileAsync("/bin/sh", [installerPath], { env: installEnv });
        const shellRc = await readFile(join(home, ".zshrc"), "utf8");
        assert.match(shellRc, /export PATH="\$HOME\/\.local\/bin:\$PATH"/);
        assert.equal(shellRc.match(/open-compute/g)?.length, 1);
      });
    }
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
