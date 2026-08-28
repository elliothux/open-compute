import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { join } from "node:path";
import { tmpdir } from "node:os";
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
