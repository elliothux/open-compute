import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { hostTarget, loadPin, sha256 } from "../scripts/workerd-archive.ts";

const root = fileURLToPath(new URL("../", import.meta.url));

test("bundled archives are atomic, bounded and reject corrupt or unhydrated inputs", () => {
  const source = `
    import assert from "node:assert/strict";
    import { mkdtemp, mkdir, writeFile, readFile, chmod, symlink, unlink, truncate, rm } from "node:fs/promises";
    import { join } from "node:path";
    import { tmpdir } from "node:os";
    import { gunzipSync } from "node:zlib";
    import { bundledWorkerdArchive, compressWorkerd } from ${JSON.stringify(new URL("../scripts/bundled-workerd.ts", import.meta.url).href)};
    import { loadPin, sha256 } from ${JSON.stringify(new URL("../scripts/workerd-archive.ts", import.meta.url).href)};
    const root = await mkdtemp(join(tmpdir(), "oc-bundled-fixture-"));
    try {
      const target = "darwin-arm64";
      const source = join(root, "share/workerd", target);
      await mkdir(source, { recursive: true });
      const path = join(source, "workerd");
      const bytes = Buffer.from("compression fixture, never executed");
      await writeFile(path, bytes, { mode: 0o755 });
      const compressed = compressWorkerd(bytes);
      assert.deepEqual([...compressed.subarray(4, 8)], [0, 0, 0, 0]);
      assert.equal(compressed[9], 255);
      const pin = { ...await loadPin(target), binarySha256: sha256(bytes), archiveSha256: sha256(compressed) };
      const [first, second] = await Promise.all([
        bundledWorkerdArchive(root, pin), bundledWorkerdArchive(root, pin),
      ]);
      assert.equal(first, second);
      assert.deepEqual(gunzipSync(await readFile(first)), bytes);
      assert.equal(await bundledWorkerdArchive(root, pin), first);
      await writeFile(path, "version https://git-lfs.github.com/spec/v1\\n");
      await assert.rejects(bundledWorkerdArchive(root, pin), /hydrate Git LFS/);
      assert.deepEqual(await readFile(first), compressed);
      await writeFile(path, bytes);
      await truncate(path, 256 * 1024 * 1024 + 1);
      await assert.rejects(bundledWorkerdArchive(root, pin), /bounded executable/);
      await unlink(path);
      await symlink(first, path);
      await assert.rejects(bundledWorkerdArchive(root, pin), /regular file/);
      await unlink(path);
      await writeFile(path, bytes, { mode: 0o755 });
      await chmod(first, 0o644);
      await writeFile(first, "corrupt-cache");
      await assert.rejects(bundledWorkerdArchive(root, pin), /cached workerd archive/);
      assert.equal(await readFile(first, "utf8"), "corrupt-cache");
    } finally { await rm(root, { recursive: true, force: true }); }
  `;
  const result = spawnSync("bun", ["--eval", source], {
    cwd: root,
    encoding: "utf8",
    timeout: 30_000,
  });
  assert.equal(result.status, 0, result.stderr);
});

test("default input preparation verifies the actual bundled host binary offline", async () => {
  const directory = await mkdtemp(join(tmpdir(), "oc-bundled-host-"));
  try {
    const destination = join(directory, "prepared");
    const result = spawnSync(
      "bun",
      ["scripts/prepare-workerd.ts", "--dest", destination],
      {
        cwd: root,
        encoding: "utf8",
        timeout: 30_000,
      },
    );
    assert.equal(result.status, 0, result.stderr);
    const pin = await loadPin();
    assert.equal(
      sha256(await readFile(join(destination, "workerd"))),
      pin.binarySha256,
    );
    assert.equal(
      sha256(await readFile(join(destination, pin.archiveName))),
      pin.archiveSha256,
    );
    assert.equal(
      sha256(
        await readFile(join(root, "share/workerd", hostTarget(), "workerd")),
      ),
      pin.binarySha256,
    );
    assert.match(result.stdout, /OPEN_COMPUTE_TEST_WORKERD=/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});
