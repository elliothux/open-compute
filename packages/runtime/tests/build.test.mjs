import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, mkdtemp, readFile, readdir, rm, stat, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const buildScript = fileURLToPath(new URL("../build.ts", import.meta.url));
function build(directory, ...args) {
  return spawnSync(process.execPath, [buildScript, "--output-dir", directory, ...args], {
    encoding: "utf8", timeout: 30_000,
  });
}

test("runtime assets are reproducible and stale or unexpected output fails closed", async t => {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-runtime-build-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const first = build(directory);
  assert.equal(first.status, 0, first.stderr);
  const names = (await readdir(directory, { recursive: true })).filter(name => name.endsWith(".js")).sort();
  assert.ok(names.includes("kv/transport.js"));
  assert.ok(names.includes("loader/snapshot.js"));
  assert.match(await readFile(join(directory, "loader/host.js"), "utf8"), /from "\.\.\/kv\/transport.js"/);
  assert.match(await readFile(join(directory, "workflows/host.js"), "utf8"), /from "\.\.\/loader\/host.js"/);
  const contents = await Promise.all(names.map(name => readFile(join(directory, name))));
  const manifestBytes = await readFile(join(directory, "manifest.json"));
  const manifest = JSON.parse(manifestBytes);
  assert.equal(manifest.schemaVersion, 1);
  assert.deepEqual(Object.keys(manifest.sources), names);
  for (const [index, name] of names.entries()) {
    assert.equal(manifest.sources[name], createHash("sha256").update(contents[index]).digest("hex"));
  }
  const secondDirectory = await mkdtemp(join(tmpdir(), "open-compute-runtime-reproduce-"));
  t.after(() => rm(secondDirectory, { recursive: true, force: true }));
  const second = build(secondDirectory);
  assert.equal(second.status, 0, second.stderr);
  assert.deepEqual((await readdir(secondDirectory, { recursive: true })).sort(),
    (await readdir(directory, { recursive: true })).sort());
  assert.deepEqual(await Promise.all(names.map(name => readFile(join(secondDirectory, name)))), contents);
  assert.deepEqual(await readFile(join(secondDirectory, "manifest.json")), manifestBytes);
  assert.equal(build(directory, "--check").status, 0);

  const mtime = (await stat(join(directory, "manifest.json"))).mtimeMs;
  assert.equal(build(directory).status, 0);
  assert.equal((await stat(join(directory, "manifest.json"))).mtimeMs, mtime);

  const asset = join(directory, names[0]);
  await writeFile(asset, "stale");
  const stale = build(directory, "--check");
  assert.notEqual(stale.status, 0);
  assert.match(stale.stderr, /stale runtime asset/);
  assert.equal(await readFile(asset, "utf8"), "stale");

  const rebuilt = build(directory);
  assert.equal(rebuilt.status, 0, rebuilt.stderr);
  assert.deepEqual(await readFile(asset), contents[0]);

  const retiredName = "retired.js";
  const retiredPath = join(directory, retiredName);
  const retiredContent = "// Generated from packages/runtime/src/retired.ts by Rolldown. Do not edit.\nexport {};\n";
  const previous = {
    ...manifest,
    sources: { ...manifest.sources, [retiredName]: createHash("sha256").update(retiredContent).digest("hex") },
  };
  const previousBytes = JSON.stringify(previous);
  await writeFile(retiredPath, retiredContent);
  await writeFile(join(directory, "manifest.json"), previousBytes);
  assert.notEqual(build(directory, "--check").status, 0);
  assert.equal(await readFile(retiredPath, "utf8"), retiredContent);
  await writeFile(retiredPath, `${retiredContent}// modified locally\n`);
  const modifiedRetired = build(directory);
  assert.notEqual(modifiedRetired.status, 0);
  assert.match(modifiedRetired.stderr, /unexpected runtime asset/);
  assert.equal(await readFile(retiredPath, "utf8"), `${retiredContent}// modified locally\n`);
  assert.equal(await readFile(join(directory, "manifest.json"), "utf8"), previousBytes);
  await writeFile(retiredPath, retiredContent);
  const pruned = build(directory);
  assert.equal(pruned.status, 0, pruned.stderr);
  assert.ok(!(await readdir(directory)).includes(retiredName));
  assert.deepEqual(await readFile(join(directory, "manifest.json")), manifestBytes);

  await writeFile(join(directory, "unexpected.js"), "unrelated");
  const unexpected = build(directory);
  assert.notEqual(unexpected.status, 0);
  assert.match(unexpected.stderr, /unexpected runtime asset/);
  assert.deepEqual(await readFile(asset), contents[0]);
  assert.ok(!(await readdir(directory, { recursive: true })).some(name => name.endsWith(".tmp")));

  const missing = build(join(directory, "absent"), "--check");
  assert.notEqual(missing.status, 0);
});

test("runtime builder rejects symlinked assets without changing their targets", async t => {
  const directory = await mkdtemp(join(tmpdir(), "open-compute-runtime-links-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const target = `${directory}.txt`;
  t.after(() => rm(target, { force: true }));
  await writeFile(target, "retain");
  await mkdir(join(directory, "gateway"));
  await symlink(target, join(directory, "gateway/ingress.js"));
  const result = build(directory);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /runtime asset must be a regular file/);
  assert.equal(await readFile(target, "utf8"), "retain");
  assert.deepEqual((await readdir(directory, { recursive: true })).sort(), ["gateway", "gateway/ingress.js"]);
});
