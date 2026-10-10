import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readdir,
  readFile,
  rm,
  stat,
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
  parseSdkPackageReport,
  releaseTargets,
  stableVersionFromTag,
  workspaceVersion,
} from "../scripts/assemble-release.ts";
import { loadCaddyPin, prepareCaddy } from "../scripts/caddy-archive.ts";
import { verifyReleaseExecutable } from "../scripts/verify-release-executable.ts";
import {
  absoluteDestination,
  cargoTargetDirectory,
  hostTarget,
  loadPin,
  prepareWorkerd,
  sha256,
  sourceArguments,
} from "../scripts/workerd-archive.ts";
import { createReleaseEvidence } from "./fixtures/release-evidence.mjs";

const execFileAsync = promisify(execFile);
const installerPath = fileURLToPath(
  new URL("../scripts/install.sh", import.meta.url),
);
const releaseWorkflowPath = fileURLToPath(
  new URL("../.github/workflows/release.yml", import.meta.url),
);
const recoveryWorkflowPath = fileURLToPath(
  new URL("../.github/workflows/release-recovery.yml", import.meta.url),
);
const ciWorkflowPath = fileURLToPath(
  new URL("../.github/workflows/ci.yml", import.meta.url),
);
const dryRunWorkflowPath = fileURLToPath(
  new URL("../.github/workflows/release-dry-run.yml", import.meta.url),
);
const localDryRunPath = fileURLToPath(
  new URL("../scripts/release-dry-run.sh", import.meta.url),
);
const localDryRunDockerfilePath = fileURLToPath(
  new URL("./release-dry-run/Dockerfile", import.meta.url),
);
const cargoConfigPath = fileURLToPath(
  new URL("../.cargo/config.toml", import.meta.url),
);

async function writeTestCommand(directory, name, source) {
  await writeFile(join(directory, name), source, { mode: 0o755 });
}

async function writeTargetCommands(directory) {
  await writeTestCommand(
    directory,
    "id",
    '#!/bin/sh\ncase "$1" in\n  -u) printf \'%s\\n\' "${OPEN_COMPUTE_TEST_UID:-1000}" ;;\n  -P) printf \'test:*:1000:1000::0:0:Test:%s:/bin/sh\\n\' "${OPEN_COMPUTE_TEST_USER_HOME:-$HOME}" ;;\n  *) exit 2 ;;\nesac\n',
  );
  await writeTestCommand(
    directory,
    "getent",
    '#!/bin/sh\n[ "$1" = "passwd" ] || exit 2\nprintf \'test:x:%s:1000:Test:%s:/bin/sh\\n\' "$2" "${OPEN_COMPUTE_TEST_USER_HOME:-$HOME}"\n',
  );
  await writeTestCommand(
    directory,
    "stat",
    '#!/bin/sh\nif { [ "$1" = "-f" ] && [ "$2" = "%Lp" ]; } || { [ "$1" = "-c" ] && [ "$2" = "%a" ]; }; then printf "700\\n"; else exec /usr/bin/stat "$@"; fi\n',
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

test("build inputs require an explicit release asset source and a pinned supported host", async () => {
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
  assert.match(
    pin.archiveUrl,
    /^https:\/\/github\.com\/elliothux\/workerd\/releases\/download\//,
  );
  assert.match(
    (await loadCaddyPin()).archiveUrl,
    /^https:\/\/github\.com\/elliothux\/open-compute-caddy\/releases\/download\//,
  );
  const directory = await mkdtemp(join(tmpdir(), "oc-explicit-runtime-"));
  try {
    await assert.rejects(
      prepareWorkerd(directory, undefined, false),
      /explicitly use --download/,
    );
    assert.deepEqual(await readdir(directory), []);
  } finally {
    await rm(directory, { recursive: true, force: true });
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

test("release Cargo targets default locally and require an absolute non-root override", () => {
  assert.equal(
    cargoTargetDirectory(undefined),
    fileURLToPath(new URL("../target", import.meta.url)),
  );
  assert.equal(
    cargoTargetDirectory("/tmp/release-target"),
    "/tmp/release-target",
  );
  for (const path of ["relative", "/"])
    assert.throws(() => cargoTargetDirectory(path));
});

test("wrong archives fail without download, execution, or publication", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-hash-test-"));
  try {
    const archive = join(root, "wrong.gz");
    await writeFile(archive, "not a formal archive");
    await assert.rejects(prepareWorkerd(root, archive, false), /SHA-256/);
    await assert.rejects(prepareWorkerd(root, archive, true), /at most one/);
    await assert.rejects(prepareCaddy(root, archive, false), /SHA-256/);
    await assert.rejects(prepareCaddy(root, archive, true), /at most one/);
    await assert.rejects(readFile(join(root, "workerd")));
    await assert.rejects(readFile(join(root, "caddy")));
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

test("release qualification and local Docker diagnostic keep their exact boundaries", async () => {
  const workflow = await readFile(releaseWorkflowPath, "utf8");
  const recovery = await readFile(recoveryWorkflowPath, "utf8");
  const ci = await readFile(ciWorkflowPath, "utf8");
  const localDryRun = await readFile(localDryRunPath, "utf8");
  const localDryRunDockerfile = await readFile(
    localDryRunDockerfilePath,
    "utf8",
  );
  const cargoConfig = await readFile(cargoConfigPath, "utf8");
  await assert.rejects(stat(dryRunWorkflowPath), { code: "ENOENT" });
  await execFileAsync("bash", ["-n", localDryRunPath]);
  assert.equal(
    cargoConfig,
    '[env]\nTESSERACT_RS_CACHE_DIR = { value = "share/xberg-tesseract-cache", relative = true, force = true }\n',
  );
  assert.match(
    workflow,
    /  failfast:\n    runs-on: ubuntu-24\.04\n    environment: release[\s\S]*?bun test\/conformance\/check\.ts --case baseline-identity[\s\S]*?node --test test\/release-tools\.test\.mjs[\s\S]*?npm whoami/,
  );
  assert.match(
    workflow,
    /release_head="\$\(git rev-parse refs\/remotes\/origin\/release\)"[\s\S]*?if \[ "\$GITHUB_SHA" != "\$release_head" \]/,
  );
  assert.doesNotMatch(workflow, /merge-base --is-ancestor "\$GITHUB_SHA"/);
  assert.match(workflow, /^cache-mode: read$/m);
  assert.match(workflow, /  coverage:\n    needs: failfast\n/);
  assert.match(
    workflow,
    /name: Enforce 90 percent Rust line coverage\n\s+env:\n\s+CARGO_BUILD_JOBS: "2"\n\s+OPEN_COMPUTE_COVERAGE_HTML: "0"\n\s+run: \.\/test\/coverage\.sh --jobs 2/,
  );
  assert.match(workflow, /  integration:\n    needs: failfast\n/);
  assert.match(
    workflow,
    /  sdk-package:\n    # Build the SDK tarball once[\s\S]*?needs: failfast\n/,
  );
  assert.match(
    ci,
    /  failfast:\n    runs-on: ubuntu-24\.04[\s\S]*?Classify changed files[\s\S]*?bun test\/conformance\/check\.ts --case baseline-identity[\s\S]*?node --test test\/stress\.test\.mjs/,
  );
  assert.doesNotMatch(
    ci,
    /node --test[^\n]*test\/release-(?:tools|test-report)\.test\.mjs/,
  );
  for (const suite of ["core", "clippy", "production"]) {
    assert.match(ci, new RegExp(`- suite: ${suite}\\n`));
  }
  for (const command of [
    "./test/check-rust-clippy.sh",
    "mbx check --workspace --no-default-features",
    "mbx +1.98.0 check --workspace --all-targets",
    "./test/check-production.py",
  ]) {
    assert.equal(ci.split(command).length - 1, 1);
  }
  assert.match(
    ci,
    /  s3-provider-qualification:\n[\s\S]*?environment: s3-provider-qualification[\s\S]*?OPEN_COMPUTE_TEST_R2_S3_ACCESS_KEY_ID: \$\{\{ secrets\.OPEN_COMPUTE_TEST_R2_S3_ACCESS_KEY_ID \}\}[\s\S]*?\.\/test\/gate\.py s3-provider-qualification --jobs 1/,
  );
  assert.match(
    ci,
    /test "\$\{\{ needs\.s3-provider-qualification\.result \}\}" = success/,
  );
  assert.match(
    workflow,
    /Fetch locked crates for offline packaged-binary tests\n\s+run: mbx fetch --locked/,
  );
  assert.doesNotMatch(workflow, /v3-release-|actions\/cache\/save@/);
  const rustSetup = await readFile(
    new URL(
      "../.github/actions/setup-open-compute/action.yml",
      import.meta.url,
    ),
    "utf8",
  );
  assert.match(rustSetup, /jdx\/mr-boxington-action@v1/);
  assert.match(rustSetup, /github-cache-mode: target/);
  assert.match(
    rustSetup,
    /uses: jdx\/mr-boxington-action@v1\n\s+env:\n\s+MBX_BUILD_SCRIPT_EXECUTION: "0"\n\s+MBX_TARGET_VIEWS: "0"\n\s+MBX_RESTORE_HARDLINK: "0"/,
  );
  assert.match(
    rustSetup,
    /cache-key-suffix: \$\{\{ github\.job \}\}-\$\{\{ matrix\.suite \|\| 'build' \}\}/,
  );
  assert.match(rustSetup, /version: 1\.22\.0/);
  assert.doesNotMatch(
    workflow + ci + rustSetup,
    /Swatinem\/rust-cache|sccache|rust-cache-key|rust-cache-save/,
  );
  assert.match(
    workflow,
    /unset CARGO_TARGET_DIR[\s\S]*?OPEN_COMPUTE_TEST_OCD="\$destination"[\s\S]*?OPEN_COMPUTE_PACKAGE_GATE_USER_ROOT=1[\s\S]*?\.\/test\/gate\.py single-binary --jobs 1/,
  );
  assert.match(workflow, /path: \.temp\/release-target\/cargo-timings\//);
  assert.match(
    workflow,
    /name: unverified-native-build-\$\{\{ matrix\.target \}\}[\s\S]*?\.temp\/dashboard-e2e[\s\S]*?\.temp\/dashboard-server[\s\S]*?apps\/dashboard\/test-results/,
  );
  assert.equal(
    workflow.match(/\.\/test\/gate\.py --workspace --final --jobs 2/g)?.length,
    1,
  );
  assert.match(workflow, /test-p0-2-egress-linux\.sh p0-2 --jobs 2/);
  assert.doesNotMatch(workflow, /test-p0-2-egress-linux\.sh --workspace/);
  for (const source of [workflow]) {
    assert.match(
      source,
      /uid_home="\$\(getent passwd "\$\(id -u\)" \| cut -d: -f6\)"[\s\S]*?OPEN_COMPUTE_OCD_BIN="\$candidate"[\s\S]*?OPEN_COMPUTE_DEV_PRODUCTION_SCOPE=1[\s\S]*?OPEN_COMPUTE_DEV_OCD_ROOT="\$uid_home\/\.open-compute"[\s\S]*?\.\/scripts\/dev-test\.sh run/,
    );
    assert.match(
      source,
      /apps\/dashboard\/scripts\/run-e2e\.sh \\\n\s+dashboard\.spec\.ts lifecycle\.spec\.ts \\\n\s+--grep 'sign in survives page reload within the same tab\|Worker create, detail, and deletion use the browser SDK'/,
    );
    assert.doesNotMatch(source, /bun run test:dashboard:e2e/);
    assert.match(
      source,
      /server_evidence="\$GITHUB_WORKSPACE\/\.temp\/dashboard-server"/,
    );
    assert.match(source, /OPEN_COMPUTE_DEV_STATE_DIR="\$server_evidence"/);
  }

  assert.equal(
    localDryRun.match(/\.\/scripts\/package-release\.sh/g)?.length,
    1,
  );
  assert.equal(
    localDryRun.match(/\.\/test\/gate\.py single-binary --jobs 1/g)?.length,
    1,
  );
  assert.match(
    localDryRun,
    /OPEN_COMPUTE_CAFFEINATED=1[\s\S]*?exec \/usr\/bin\/caffeinate -is "\$0"/,
  );
  assert.match(
    localDryRun,
    /export RUSTFLAGS='-D warnings'[\s\S]*?export CARGO_NET_OFFLINE=true[\s\S]*?mbx fetch --locked --offline[\s\S]*?bun test\/conformance\/check\.ts --case baseline-identity[\s\S]*?node --test test\/release-tools\.test\.mjs/,
  );
  assert.match(
    localDryRun,
    /--archive "\$OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE"[\s\S]*?report\.target !== "linux-arm64"[\s\S]*?report\.sha256 !== process\.env\.SHA256/,
  );
  assert.match(
    localDryRun,
    /OPEN_COMPUTE_TEST_OCD="\$destination"[\s\S]*?OPEN_COMPUTE_PACKAGE_GATE_USER_ROOT=1[\s\S]*?\.\/test\/gate\.py single-binary --jobs 1/,
  );
  assert.match(
    localDryRun,
    /OPEN_COMPUTE_OCD_BIN="\$destination"[\s\S]*?OPEN_COMPUTE_DEV_PRODUCTION_SCOPE=1[\s\S]*?OPEN_COMPUTE_DEV_OCD_ROOT="\$HOME\/\.open-compute"[\s\S]*?\.\/scripts\/dev-test\.sh run/,
  );
  assert.match(
    localDryRun,
    /apps\/dashboard\/scripts\/run-e2e\.sh \\\n+\s+dashboard\.spec\.ts lifecycle\.spec\.ts \\\n+\s+--grep 'sign in survives page reload within the same tab\|Worker create, detail, and deletion use the browser SDK'/,
  );
  assert.match(
    localDryRun,
    /docker run "\$\{container_args\[@\]\}" \\\n+\s+--network bridge[\s\S]*?--inside hydrate[\s\S]*?docker run "\$\{container_args\[@\]\}" \\\n+\s+--network none[\s\S]*?--inside qualify/,
  );
  assert.match(
    localDryRun,
    /--mount "type=bind,src=\$host_output,dst=\$container_output"[\s\S]*?OPEN_COMPUTE_RELEASE_DRY_RUN_OUTPUT=\$container_output/,
  );
  assert.match(
    localDryRun,
    /--iidfile "\$iid_file"[\s\S]*?grep -Eq '\^sha256:\[0-9a-f\]\{64\}\$' "\$iid_file"[\s\S]*?image_id=\$\(<"\$iid_file"\)/,
  );
  assert.equal(localDryRun.match(/"\$image_id"/g)?.length, 3);
  assert.match(
    localDryRun,
    /host_uid=\$\(id -u\)[\s\S]*?host_gid=\$\(id -g\)[\s\S]*?--user "\$host_uid:\$host_gid"[\s\S]*?src=\$passwd_file,dst=\/etc\/passwd,readonly[\s\S]*?uid=\$host_uid,gid=\$host_gid/,
  );
  assert.doesNotMatch(localDryRun, /CARGO_BUILD_JOBS|--user 1001:1001/);
  for (const option of [
    "--pull=never",
    "--read-only",
    "--cap-drop ALL",
    "--security-opt no-new-privileges",
  ]) {
    assert.match(localDryRun, new RegExp(option));
  }
  assert.doesNotMatch(
    localDryRun,
    /playwright install|docker\.sock|--privileged|--network host|npm publish|git (?:push|tag)/,
  );
  assert.match(
    localDryRun,
    /if \[ "\$phase" = hydrate \]; then[\s\S]*?prepare-workerd\.ts[^\n]*--download[\s\S]*?prepare-caddy\.ts[^\n]*--download[\s\S]*?return[\s\S]*?\[ "\$phase" = qualify \]/,
  );
  assert.doesNotMatch(localDryRun, /test -[nz] "\$\(git status/);
  assert.deepEqual(localDryRunDockerfile.match(/^FROM .*$/gm), [
    "FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS rust",
    "FROM oven/bun:1.3.14@sha256:e10577f0db68676a7024391c6e5cb4b879ebd17188ab750cf10024a6d700e5c4 AS bun",
    "FROM node:26.8.1-bookworm-slim@sha256:367679cf9792759492a486e4aa4b421764d71a9546a6dae8aab81a99eb797b3e AS node",
    "FROM mcr.microsoft.com/playwright:v1.63.0-noble@sha256:eff16c30e6f3f4af0a03fa4b706120d5e9b0891c344a27d64559aff5900a4a27",
  ]);
  assert.match(localDryRunDockerfile, /^USER pwuser$/m);
  assert.doesNotMatch(
    localDryRunDockerfile,
    /package-release|gate\.py|release-dry-run\.sh/,
  );

  assert.doesNotMatch(workflow, /\n  (?:msrv|lint-test):\n/);
  // npm publication is token-authenticated and never receives the GitHub
  // token; the tarball is published from the verified artifact only.
  assert.match(
    workflow,
    /NPM_ACCESS_TOKEN: \$\{\{ secrets\.NPM_ACCESS_TOKEN \}\}/,
  );
  assert.doesNotMatch(workflow, /NPM_TOKEN/);
  assert.match(workflow, /npm publish "\$tarball" --access public/);
  assert.match(
    workflow,
    /if ! npm publish "\$tarball"[\s\S]*?npm view "@open-compute\/sdk@\$RELEASE_VERSION" dist\.shasum/,
  );
  // Publication is never retried. OS fixtures may wait for real service readiness.
  for (const source of [
    workflow.slice(workflow.indexOf("\n  publish:")),
    recovery,
  ]) {
    assert.doesNotMatch(source, /for attempt in|sleep 5/);
  }
  assert.match(workflow, /--draft=false/);
  assert.doesNotMatch(workflow, /--provenance/);
  assert.doesNotMatch(workflow, /tolerate-republish/);
});

test("recovery rebuilds the stress index without accepting failed or unknown results", async () => {
  const workflow = await readFile(recoveryWorkflowPath, "utf8");
  const qualification = workflow.match(/--json jobs --jq '([\s\S]*?)'/)?.[1];
  assert.ok(qualification);
  const names = [
    "failfast",
    "coverage",
    "sdk-package",
    "stress",
    "integration (macos)",
    "integration (linux)",
    "package (macos)",
    "package (arm64)",
    "package (x64)",
    ...Array.from({ length: 5 }, (_, index) => `install-lifecycle (${index})`),
  ];
  const jobs = names.map((name, databaseId) => ({
    name,
    databaseId,
    conclusion: "success",
  }));
  for (const [items, expected] of [
    [jobs, "success"],
    [jobs.slice(1), "failed"],
    [[{ ...jobs[0], conclusion: "failure" }, ...jobs.slice(1)], "failed"],
    [
      [...jobs, { ...jobs[0], databaseId: 100, conclusion: "failure" }],
      "failed",
    ],
  ]) {
    const { stdout } = await execFileAsync("jq", [
      "-nr",
      "--argjson",
      "evidence",
      JSON.stringify({ jobs: items }),
      `$evidence | ${qualification}`,
    ]);
    assert.equal(stdout.trim(), expected);
  }
  assert.match(
    workflow,
    /\.head_sha[\s\S]*?git rev-parse "refs\/tags\/\$RELEASE_TAG\^\{\}"/,
  );
  const script = workflow
    .match(/          python3 - <<'PY'\n([\s\S]*?)          PY/)?.[1]
    .replace(/^          /gm, "");
  assert.ok(script);
  const directory = await mkdtemp(join(tmpdir(), "oc-recovery-"));
  try {
    const evidence = join(
      directory,
      ".temp/release-evidence/stress-qualification",
    );
    await mkdir(evidence, { recursive: true });
    const path = join(evidence, "qualification.json");
    const profiles = [
      "smoke",
      "p0-2c4g",
      "scenario",
      "p1-2c4g-peak",
      "p1-2c4g-soak",
    ];
    const events = [{ event: "restart_reconcile_ok" }];
    const runs = profiles.map((profile) => ({
      profile,
      verdict: "pass",
      global_anomalies: [],
      ...(profile.endsWith("soak") ? { soak: { events } } : {}),
    }));
    const reconcile = {
      profile: "reconcile",
      verdict: "pass",
      global_anomalies: [],
      stacks: { kv: { verdict: "pass", anomalies: [] } },
      scenario: {},
    };
    const original = {
      revision: "fixture",
      runs: [...runs, reconcile, reconcile, reconcile],
    };
    await writeFile(path, JSON.stringify(original));
    await execFileAsync("python3", ["-c", script], { cwd: directory });
    assert.deepEqual(JSON.parse(await readFile(path, "utf8")), {
      revision: "fixture",
      runs,
    });
    for (const invalid of [
      { ...reconcile, verdict: "fail" },
      { ...reconcile, profile: "unknown" },
      { ...reconcile, global_anomalies: ["failure"] },
      { ...reconcile, stacks: { kv: { verdict: "fail", anomalies: [] } } },
      runs[0],
    ]) {
      await writeFile(path, JSON.stringify({ runs: [...runs, invalid] }));
      await assert.rejects(
        execFileAsync("python3", ["-c", script], { cwd: directory }),
      );
    }
    await writeFile(path, JSON.stringify({ runs: runs.slice(1) }));
    await assert.rejects(
      execFileAsync("python3", ["-c", script], { cwd: directory }),
    );
  } finally {
    await rm(directory, { recursive: true });
  }
});

test("release assembly requires and describes the exact three native executables", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-assembly-test-"));
  const evidenceDirectory = await mkdtemp(
    join(tmpdir(), "oc-release-evidence-test-"),
  );
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
  const sdkReport = {
    schemaVersion: 1,
    package: "@open-compute/sdk",
    packageVersion: "1.2.3",
    tarball: "open-compute-sdk-1.2.3.tgz",
    tarballShasum: "b".repeat(40),
    tarballIntegrity: "sha512-cdkovenkZmV2ZGVk",
    surfaceDigest: "c".repeat(64),
    openapiRevision: "d".repeat(40),
    cloudflareSdkVersion: "7.2.0",
    files: ["package/package.json"],
  };
  assert.deepEqual(parseSdkPackageReport(sdkReport), sdkReport);
  assert.throws(
    () => parseSdkPackageReport({ ...sdkReport, schemaVersion: 2 }),
    /schema/,
  );
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
    await createReleaseEvidence(
      evidenceDirectory,
      identity,
      sha256(Buffer.from("native-linux-x64")),
    );
    const badReportPath = join(root, "release-report-linux-x64.json");
    const badReport = JSON.parse(await readFile(badReportPath, "utf8"));
    badReport.revision = "f".repeat(40);
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assert.rejects(
      assembleRelease(root, "v1.2.3", identity, sdkReport, evidenceDirectory),
      /does not match/,
    );
    badReport.revision = identity.revision;
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assembleRelease(
      root,
      "v1.2.3",
      identity,
      sdkReport,
      evidenceDirectory,
    );
    assert.deepEqual(
      (await readdir(root)).sort(),
      [
        "SHA256SUMS",
        ...releaseTargets.map((target) => `ocd-v1.2.3-${target}`),
        ...releaseTargets.map((target) => `release-report-${target}.json`),
        "release.json",
        "test-report.json",
        "test-report.html",
      ].sort(),
    );
    const manifest = JSON.parse(
      await readFile(join(root, "release.json"), "utf8"),
    );
    assert.equal(manifest.schemaVersion, 1);
    assert.equal(manifest.tag, "v1.2.3");
    assert.equal(manifest.gitRevision, identity.revision);
    assert.deepEqual(manifest.sdk, {
      package: "@open-compute/sdk",
      packageVersion: "1.2.3",
      tarballShasum: "b".repeat(40),
      tarballIntegrity: "sha512-cdkovenkZmV2ZGVk",
      surfaceDigest: "c".repeat(64),
      openapiRevision: "d".repeat(40),
      cloudflareSdkVersion: "7.2.0",
    });
    assert.deepEqual(
      manifest.artifacts.map((artifact) => artifact.target),
      releaseTargets,
    );
    const checksums = await readFile(join(root, "SHA256SUMS"), "utf8");
    assert.equal(checksums.trim().split("\n").length, 6);
    assert.match(checksums, /  release\.json$/m);
    await assert.rejects(
      assembleRelease(root, "v1.2.3", identity, sdkReport, evidenceDirectory),
      /exact three binaries/,
    );
  } finally {
    await rm(evidenceDirectory, { recursive: true, force: true });
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
    */.open-compute) exit 73 ;;
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
          SUDO_UID: "1000",
          SUDO_GID: "1000",
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

test("installer selects the running UID home rather than an overridden HOME", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-install-home-scope-test-"));
  const commands = join(root, "commands");
  const ownerHome = join(root, "owner");
  await mkdir(commands);
  await writeTargetCommands(commands);
  await writeTestCommand(commands, "curl", "#!/bin/sh\nexit 99\n");
  try {
    await assert.rejects(
      execFileAsync("/bin/sh", [installerPath], {
        env: {
          ...process.env,
          HOME: join(root, "foreign-home"),
          OPEN_COMPUTE_TEST_USER_HOME: ownerHome,
          OPEN_COMPUTE_RELEASE_TAG: "v1.2.3",
          OPEN_COMPUTE_TEST_OS: "Linux",
          OPEN_COMPUTE_TEST_ARCH: "x86_64",
          PATH: `${commands}:${process.env.PATH ?? ""}`,
        },
      }),
      /failed to download release.json/,
    );
    assert.equal(
      (await stat(join(ownerHome, ".open-compute"))).mode & 0o777,
      0o700,
    );
    await assert.rejects(
      stat(join(root, "foreign-home/.open-compute")),
      /ENOENT/,
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
        const receiptPath = join(home, ".open-compute/install-receipt.json");
        assert.equal(await readFile(destination, "utf8"), binary.toString());
        const receipt = JSON.parse(await readFile(receiptPath, "utf8"));
        assert.equal((await stat(receiptPath)).mode & 0o777, 0o600);
        assert.equal(
          (await stat(join(home, ".open-compute"))).mode & 0o777,
          0o700,
        );
        assert.deepEqual(await readdir(join(home, ".open-compute/tmp")), []);
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

test("installation qualification is a mandatory publication dependency", async () => {
  const workflow = await readFile(releaseWorkflowPath, "utf8");
  const ci = await readFile(ciWorkflowPath, "utf8");
  assert.match(
    workflow,
    /  publish:\n    needs: \[[^\n]*install-lifecycle[^\n]*\]/,
  );
  assert.match(
    workflow,
    /  install-lifecycle:\n    needs: \[failfast, package\]/,
  );
  for (const name of [
    "ubuntu-24-x64",
    "ubuntu-24-arm64",
    "debian-13-x64",
    "debian-13-arm64",
    "macos-arm64",
  ]) {
    assert.match(workflow, new RegExp(`- name: ${name}\\n`));
  }
  assert.match(workflow, /for scope in user system/);
  assert.match(
    ci,
    /mbx test --locked -p open-compute-service --lib service_manager::tests/,
  );
  const recovery = await readFile(recoveryWorkflowPath, "utf8");
  assert.match(recovery, /startswith\("install-lifecycle \("\)/);
  assert.match(recovery, /length == 14 and all\(\.conclusion == "success"\)/);
  await execFileAsync(
    "python3",
    ["-B", "-m", "unittest", "discover", "-s", "test/install-lifecycle"],
    {
      cwd: fileURLToPath(new URL("../", import.meta.url)),
    },
  );
});
