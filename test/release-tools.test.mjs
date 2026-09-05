import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
import { verifyReleaseExecutable } from "../scripts/verify-release-executable.ts";
import { assembleRelease, releaseTargets, stableVersionFromTag, workspaceVersion } from "../scripts/assemble-release.ts";
import { absoluteDestination, hostTarget, loadPin, prepareWorkerd, sha256, sourceArguments } from "../scripts/workerd-archive.ts";

test("build inputs require one explicit source and a pinned supported host", async () => {
  assert.equal(sourceArguments(["--dest", "/tmp/new", "--archive", "/tmp/pin.gz"]).archive, "/tmp/pin.gz");
  assert.equal(sourceArguments(["--dest", "/tmp/new", "--download"]).download, true);
  for (const args of [[], ["--dest", "/tmp/new"], ["--dest", "relative", "--download"],
    ["--dest", "/tmp/new", "--archive", "/tmp/pin.gz", "--download"], ["--dest", "/tmp/new", "--download", "--download"]]) {
    assert.throws(() => sourceArguments(args));
  }
  const pin = await loadPin();
  assert.equal(pin.target, hostTarget());
  assert.match(pin.archiveSha256, /^[a-f0-9]{64}$/);
  assert.match(pin.archiveUrl, /^https:\/\/github\.com\/cloudflare\/workerd\/releases\/download\//);
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
    assert.equal(await absoluteDestination(join(root, "new")), join(root, "new"));
    assert.equal(await readFile(winner, "utf8"), "keep");
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("wrong archives fail without download, execution, or publication", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-hash-test-"));
  try {
    const archive = join(root, "wrong.gz");
    await writeFile(archive, "not a formal archive");
    await assert.rejects(prepareWorkerd(root, archive, false), /SHA-256/);
    await assert.rejects(prepareWorkerd(root, archive, true), /exactly one/);
    await assert.rejects(prepareWorkerd(root, undefined, false), /exactly one/);
    await assert.rejects(readFile(join(root, "workerd")));
    assert.equal(sha256(Buffer.from("abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  } finally { await rm(root, { recursive: true, force: true }); }
});

test("release tags are stable SemVer and match the workspace version", () => {
  assert.equal(stableVersionFromTag("v0.1.0"), "0.1.0");
  assert.equal(workspaceVersion("[workspace]\n\n[workspace.package]\nversion = \"12.3.4\"\n\n[dependencies]\n"), "12.3.4");
  for (const tag of ["0.1.0", "v0.1", "v01.2.3", "v1.2.3-alpha.1", "v1.2.3+build"]) {
    assert.throws(() => stableVersionFromTag(tag));
  }
});

test("release assembly requires and describes the exact four native executables", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-assembly-test-"));
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
      await writeFile(join(root, `release-report-${target}.json`), JSON.stringify({
        schemaVersion: 1,
        destination: join(root, filename),
        target,
        ...identity,
        bytes: bytes.length,
        sha256: sha256(bytes),
      }));
    }
    const badReportPath = join(root, "release-report-linux-x64.json");
    const badReport = JSON.parse(await readFile(badReportPath, "utf8"));
    badReport.revision = "f".repeat(40);
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assert.rejects(assembleRelease(root, "v1.2.3", identity), /does not match/);
    badReport.revision = identity.revision;
    await writeFile(badReportPath, JSON.stringify(badReport));
    await assembleRelease(root, "v1.2.3", identity);
    assert.deepEqual((await readdir(root)).sort(), [
      "SHA256SUMS",
      ...releaseTargets.map((target) => `ocd-v1.2.3-${target}`),
      ...releaseTargets.map((target) => `release-report-${target}.json`),
      "release.json",
    ].sort());
    const manifest = JSON.parse(await readFile(join(root, "release.json"), "utf8"));
    assert.equal(manifest.schemaVersion, 1);
    assert.equal(manifest.tag, "v1.2.3");
    assert.equal(manifest.gitRevision, identity.revision);
    assert.deepEqual(manifest.artifacts.map((artifact) => artifact.target), releaseTargets);
    const checksums = await readFile(join(root, "SHA256SUMS"), "utf8");
    assert.equal(checksums.trim().split("\n").length, 5);
    assert.match(checksums, /  release\.json$/m);
    await assert.rejects(assembleRelease(root, "v1.2.3", identity), /exact four binaries/);
  } finally { await rm(root, { recursive: true, force: true }); }
});


test("release CLI verification supplies a private generated config and rejects identity drift", async () => {
  const root = await mkdtemp(join(tmpdir(), "oc-release-cli-test-"));
  try {
    const binary = join(root, "ocd-fixture");
    await writeFile(binary, `#!/usr/bin/env node
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
`, { mode: 0o755 });
    const pin = { expectedVersion: "workerd pin", lockSha256: "digest" };
    const good = join(root, "good");
    await mkdir(good);
    assert.equal(await verifyReleaseExecutable(binary, good, "revision", pin), "0.1.0");
    await assert.rejects(readFile(join(good, "data")), /ENOENT/);
    for (const [name, revision, expected] of [
      ["revision", "different", pin],
      ["pin", "revision", { ...pin, lockSha256: "different" }],
    ]) {
      const directory = join(root, name);
      await mkdir(directory);
      await assert.rejects(verifyReleaseExecutable(binary, directory, revision, expected),
        /does not match the build inputs/);
    }
  } finally { await rm(root, { recursive: true, force: true }); }
});
