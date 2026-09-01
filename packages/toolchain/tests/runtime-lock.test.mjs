import assert from "node:assert/strict";
import { mkdtemp, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { loadFormalRuntimeLock, loadFormalRuntimeLockAt } from "../src/runtime-lock.ts";

const FORMAL_LOCK = resolve(fileURLToPath(new URL("../../runtime/workerd.lock.json", import.meta.url)));

async function fixture(t, body) {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-runtime-lock-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const path = join(directory, "workerd.lock.json");
  await writeFile(path, body);
  return { directory, path };
}

async function cloneLock() {
  return JSON.parse(await readFile(FORMAL_LOCK, "utf8"));
}

test("reads consumed fields from the formal lock without copying the compatibility date", async () => {
  const lock = await loadFormalRuntimeLock();
  const raw = await cloneLock();
  assert.equal(lock.effectiveCompatibilityDate, raw.effectiveCompatibilityDate);
  assert.deepEqual(lock.requiredCompatibilityFlags, raw.requiredCompatibilityFlags);
  assert.deepEqual(lock.systemCompatibilityFlags, raw.systemCompatibilityFlags);
  assert.match(lock.effectiveCompatibilityDate, /^\d{4}-\d{2}-\d{2}$/);
});

test("reads a valid lock from an injected path", async t => {
  const raw = await cloneLock();
  const { path } = await fixture(t, JSON.stringify(raw));
  assert.deepEqual(await loadFormalRuntimeLockAt(path), await loadFormalRuntimeLock());
});

test("rejects malformed, non-Gregorian, and padded compatibility dates", async t => {
  const raw = await cloneLock();
  for (const date of ["2026-8-30", "20260830", "2026/08/30", "2026-08-30T00:00:00Z", ""]) {
    const { path } = await fixture(t, JSON.stringify({ ...raw, effectiveCompatibilityDate: date }));
    await assert.rejects(loadFormalRuntimeLockAt(path), /invalid effectiveCompatibilityDate/);
  }
  for (const date of ["2026-02-29", "2026-13-01", "2026-08-32", "2026-00-01", "2026-08-00", "1969-12-31"]) {
    const { path } = await fixture(t, JSON.stringify({ ...raw, effectiveCompatibilityDate: date }));
    await assert.rejects(loadFormalRuntimeLockAt(path), /real calendar date/);
  }
});

test("rejects malformed, duplicate, and overlapping compatibility flags", async t => {
  const raw = await cloneLock();
  const malformed = [
    { ...raw, requiredCompatibilityFlags: ["bad-flag"] },
    { ...raw, systemCompatibilityFlags: [""] },
    { ...raw, requiredCompatibilityFlags: ["flag with space"] },
    { ...raw, systemCompatibilityFlags: ["experimental!"] },
  ];
  for (const value of malformed) {
    const { path } = await fixture(t, JSON.stringify(value));
    await assert.rejects(loadFormalRuntimeLockAt(path), /malformed/);
  }
  const duplicated = [
    { ...raw, requiredCompatibilityFlags: ["nodejs_compat", "nodejs_compat"] },
    { ...raw, systemCompatibilityFlags: ["experimental", "experimental"] },
  ];
  for (const value of duplicated) {
    const { path } = await fixture(t, JSON.stringify(value));
    await assert.rejects(loadFormalRuntimeLockAt(path), /duplicated/);
  }
  const overlap = {
    ...raw,
    requiredCompatibilityFlags: ["experimental"],
    systemCompatibilityFlags: ["experimental", "service_binding_extra_handlers"],
  };
  const { path } = await fixture(t, JSON.stringify(overlap));
  await assert.rejects(loadFormalRuntimeLockAt(path), /disjoint/);
});

test("rejects a lock that is not the current formal schema", async t => {
  const raw = await cloneLock();
  const { path: missing } = await fixture(t, JSON.stringify({
    effectiveCompatibilityDate: raw.effectiveCompatibilityDate,
    requiredCompatibilityFlags: raw.requiredCompatibilityFlags,
    systemCompatibilityFlags: raw.systemCompatibilityFlags,
  }));
  await assert.rejects(loadFormalRuntimeLockAt(missing), /schema version/);
  const { path: wrongVersion } = await fixture(t, JSON.stringify({ ...raw, schemaVersion: 2 }));
  await assert.rejects(loadFormalRuntimeLockAt(wrongVersion), /schema version/);
  const { path: badRevision } = await fixture(t, JSON.stringify({ ...raw, revision: "not-a-git-sha" }));
  await assert.rejects(loadFormalRuntimeLockAt(badRevision), /invalid revision/);
  const { path: emptyTargets } = await fixture(t, JSON.stringify({ ...raw, targets: {} }));
  await assert.rejects(loadFormalRuntimeLockAt(emptyTargets), /at least one target/);
});

test("rejects final symlinks, non-regular, oversized, truncated, and invalid UTF-8 input", async t => {
  const raw = await cloneLock();
  const { directory, path } = await fixture(t, JSON.stringify(raw));
  const linked = join(directory, "linked.lock.json");
  await symlink(path, linked);
  await assert.rejects(loadFormalRuntimeLockAt(linked), /symbolic link/);
  await assert.rejects(loadFormalRuntimeLockAt(directory), /regular file/);
  const { path: oversized } = await fixture(t, `${JSON.stringify(raw)}${" ".repeat(64 * 1024)}`);
  await assert.rejects(loadFormalRuntimeLockAt(oversized), /64 KiB/);
  const truncated = JSON.stringify(raw).slice(0, 40);
  const { path: truncatedPath } = await fixture(t, truncated);
  await assert.rejects(loadFormalRuntimeLockAt(truncatedPath), /valid JSON/);
  const { path: utf8 } = await fixture(t, Buffer.from([0xff, 0xfe, 0xfd, 0x00]));
  await assert.rejects(loadFormalRuntimeLockAt(utf8), /UTF-8/);
});
