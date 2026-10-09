import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmod,
  mkdir,
  mkdtemp,
  readFile,
  rm,
  symlink,
  writeFile,
} from "node:fs/promises";
import { join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const repository = fileURLToPath(new URL("../", import.meta.url));
const script = join(repository, "scripts/prepare-browser.ts");

test("browser preparation rejects unsafe inputs and never overwrites an installation", async () => {
  const purpose = join(repository, ".temp/browser-preparation-tests");
  await mkdir(purpose, { recursive: true });
  const root = await mkdtemp(join(purpose, "inputs-"));
  let passed = false;
  try {
    const source = join(root, "chrome-headless-shell");
    const destination = join(root, "prepared");
    await writeFile(source, Buffer.from("\0inspector.html\0"), { mode: 0o700 });
    await mkdir(destination);
    const sentinel = join(destination, "sentinel");
    await writeFile(sentinel, "existing installation");
    for (const arguments_ of [
      [],
      ["--source", "relative", "--dest", destination],
      ["--source", source, "--dest", destination, "--download"],
      ["--source", source, "--dest", destination],
    ]) {
      const result = spawnSync(process.execPath, [script, ...arguments_], {
        cwd: repository,
        encoding: "utf8",
        timeout: 10_000,
      });
      assert.equal(result.error, undefined);
      assert.notEqual(result.status, 0);
      assert.equal(await readFile(sentinel, "utf8"), "existing installation");
    }
    // Native Linux ELF strings include the HTTP /devtools/ prefix.
    await writeFile(source, Buffer.from("\0/devtools/inspector.html\0"));
    const prefixed = spawnSync(
      process.execPath,
      [script, "--source", source, "--dest", destination],
      { cwd: repository, encoding: "utf8", timeout: 10_000 },
    );
    assert.equal(prefixed.error, undefined);
    assert.match(prefixed.stderr, /destination already exists/);
    assert.equal(await readFile(sentinel, "utf8"), "existing installation");
    await chmod(source, 0o777);
    let result = spawnSync(
      process.execPath,
      [script, "--source", source, "--dest", join(root, "unsafe")],
      { cwd: repository, encoding: "utf8", timeout: 10_000 },
    );
    assert.equal(result.error, undefined);
    assert.match(result.stderr, /unsafe browser executable/);
    await chmod(source, 0o700);
    const alias = join(root, "alias");
    await symlink(source, alias);
    result = spawnSync(
      process.execPath,
      [script, "--source", alias, "--dest", join(root, "symlink")],
      { cwd: repository, encoding: "utf8", timeout: 10_000 },
    );
    assert.equal(result.error, undefined);
    assert.match(result.stderr, /without symlinks/);
    passed = true;
  } finally {
    if (passed) await rm(root, { recursive: true });
  }
});

test("CI browser preparation verifies the archive before invoking the native preparer", async () => {
  const purpose = join(repository, ".temp/browser-preparation-tests");
  await mkdir(purpose, { recursive: true });
  const root = await mkdtemp(join(purpose, "ci-"));
  let passed = false;
  try {
    const action = await readFile(
      join(repository, ".github/actions/setup-open-compute/action.yml"),
      "utf8",
    );
    const body = action
      .split("- name: Download and verify chrome-headless-shell fixture")[1]
      .split("run: |\n")[1]
      .split("\n")
      .map((line) => line.replace(/^ {8}/, ""))
      .join("\n");
    await mkdir(join(root, "test"));
    await mkdir(join(root, "bin"));
    const archive = Buffer.from("verified fixture archive");
    await writeFile(join(root, "input"), archive);
    await writeFile(
      join(root, "test/browser-fixture.lock.json"),
      JSON.stringify({
        targets: {
          "darwin-arm64": {
            platform: "mac-arm64",
            url: "https://fixture.invalid/headless.zip",
            sha256: createHash("sha256").update(archive).digest("hex"),
          },
          "linux-x64": {
            platform: "linux64",
            url: "https://fixture.invalid/headless.zip",
            sha256: createHash("sha256").update(archive).digest("hex"),
          },
        },
      }),
    );
    const commands = {
      uname:
        'if [ "$1" = -s ]; then echo "$FIXTURE_OS"; else echo "$FIXTURE_ARCH"; fi',
      curl: 'test "$5" = https://fixture.invalid/headless.zip && cp "$PWD/input" "$4"',
      unzip:
        'mkdir -p "$4/chrome-headless-shell-$FIXTURE_PLATFORM"; touch "$4/chrome-headless-shell-$FIXTURE_PLATFORM/chrome-headless-shell"',
      bun: 'test "$1" = scripts/prepare-browser.ts && test -f "$3" && test "$4" = --dest && echo "OPEN_COMPUTE_TEST_BROWSER=$5/chrome-headless-shell"',
    };
    for (const [name, command] of Object.entries(commands))
      await writeFile(
        join(root, "bin", name),
        `#!/bin/sh\nset -eu\n${command}\n`,
        {
          mode: 0o700,
        },
      );
    const environment = join(root, "environment");
    const run = (os, arch, platform) =>
      spawnSync("bash", ["-c", body], {
        cwd: root,
        env: {
          ...process.env,
          PATH: `${join(root, "bin")}:${process.env.PATH}`,
          GITHUB_ENV: environment,
          FIXTURE_OS: os,
          FIXTURE_ARCH: arch,
          FIXTURE_PLATFORM: platform,
        },
        encoding: "utf8",
        timeout: 10_000,
      });
    for (const host of [
      ["Darwin", "arm64", "mac-arm64"],
      ["Linux", "x86_64", "linux64"],
    ]) {
      await writeFile(join(root, "input"), archive);
      const valid = run(...host);
      assert.equal(valid.error, undefined);
      assert.equal(valid.status, 0, valid.stderr);
      const prepared = await readFile(environment, "utf8");
      assert.match(
        prepared,
        /OPEN_COMPUTE_TEST_BROWSER=.*\/prepared\/chrome-headless-shell/,
      );
      await writeFile(join(root, "input"), "corrupt archive");
      const corrupt = run(...host);
      assert.equal(corrupt.error, undefined);
      assert.notEqual(corrupt.status, 0);
      assert.equal(await readFile(environment, "utf8"), prepared);
    }
    passed = true;
  } finally {
    if (passed) await rm(root, { recursive: true });
  }
});
