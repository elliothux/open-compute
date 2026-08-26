"use strict";

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const assert = require("../harness/assertions");
const {
  acquireWorkerd,
  readLock,
  sha256File,
  verifyBinary,
  G0_ROOT,
} = require("../harness/runtime");
const {
  WorkerdProcess,
  runWorkerdOnce,
  occupyEphemeral,
  closeServer,
  freePort,
  liveCount,
  WORKERD_DIR,
  ensureDir,
} = require("../harness/process-supervisor");

const INTERNAL_PATHS = [
  "/admin",
  "/debug",
  "/internal",
  "/internal/workers",
  "/loader-host",
  "/binding-host",
  "/do-supervisor",
  "/g0-do-disk",
  "/g0-fixtures",
];

const CASE_NAMES = [
  "lock-version-checksum",
  "checksum-mismatch-before-spawn",
  "config-parses-with-pinned-binary",
  "invalid-config-nonzero",
  "port-collision-fail-closed",
  "unwritable-data-dir-fail-closed",
  "health-only-after-ready",
  "default-entrypoint",
  "named-entrypoint",
  "internal-paths-not-public",
  "handler-exception-contained",
  "sigterm-exits",
  "sigkill-observed",
  "restart-new-pid",
  "harness-exit-reaps-child",
  "no-leaked-workerd-child",
];

class Reporter {
  constructor(required) {
    this.required = required;
    this.results = [];
  }

  async test(name, fn) {
    const started = Date.now();
    try {
      await fn();
      this.results.push({ name, status: "passed", ms: Date.now() - started });
      console.log(`passed          ${name}  ${Date.now() - started}ms`);
    } catch (err) {
      this.results.push({
        name,
        status: "failed",
        ms: Date.now() - started,
        error: String(err && err.message ? err.message : err),
      });
      console.log(`failed          ${name}  ${Date.now() - started}ms`);
      console.log(`  ${err && err.stack ? err.stack : err}`);
    }
  }

  na(name, reason) {
    this.results.push({ name, status: "not-applicable", reason });
    console.log(`not-applicable  ${name}`);
    console.log(`  ${reason}`);
  }

  notRun(name, reason) {
    this.results.push({ name, status: "not-run", reason: reason || "not executed" });
    console.log(`not-run         ${name}`);
    if (reason) console.log(`  ${reason}`);
  }

  summary() {
    const seen = new Set();
    const duplicates = [];
    for (const row of this.results) {
      if (seen.has(row.name)) {
        if (!duplicates.includes(row.name)) duplicates.push(row.name);
      } else {
        seen.add(row.name);
      }
    }
    for (const name of this.required) {
      if (!seen.has(name)) this.notRun(name, "never started");
    }
    if (duplicates.length > 0) {
      this.results.push({
        name: "duplicate-recorded-name",
        status: "failed",
        error: `duplicate test names: ${duplicates.join(", ")}`,
      });
    }
    const passed = this.results.filter((r) => r.status === "passed").length;
    const failed = this.results.filter((r) => r.status === "failed").length;
    const notRun = this.results.filter((r) => r.status === "not-run").length;
    const notApplicable = this.results.filter((r) => r.status === "not-applicable").length;
    const code = failed === 0 && notRun === 0 ? 0 : 1;
    console.log("");
    console.log(`results: ${passed} passed, ${failed} failed, ${notRun} not-run`);
    console.log(
      JSON.stringify({
        suite: "bootstrap",
        gates: ["G0.0", "G0.1"],
        results: this.results,
        passed,
        failed,
        notRun,
        notApplicable,
      })
    );
    return { passed, failed, notRun, notApplicable, code };
  }
}

async function probeHealth(url) {
  try {
    const res = await fetch(url);
    const text = await res.text();
    let json = null;
    try {
      json = JSON.parse(text);
    } catch {
      json = null;
    }
    return { reached: true, ok: res.ok, status: res.status, json, text };
  } catch (err) {
    return { reached: false, ok: false, status: 0, error: String(err && err.message ? err.message : err) };
  }
}

async function withWorkerd(acquired, fn, options = {}) {
  const proc = new WorkerdProcess({
    binPath: acquired.binPath,
    lock: acquired.lock,
    ...options,
  });
  try {
    await proc.start();
    const result = await fn(proc);
    const stopSignal = options.stopSignal || "SIGTERM";
    const exit = await proc.stop(stopSignal);
    await proc.cleanupSuccess();
    return { proc, result, exit };
  } catch (err) {
    const retained = proc.retainFailed(err);
    err.message = `${err.message} (retained ${retained})`;
    try {
      await proc.kill("SIGKILL");
    } catch {
      /* ignore */
    }
    throw err;
  }
}

async function run() {
  const reporter = new Reporter(CASE_NAMES);
  console.log("G0.0 pinned workerd bootstrap / G0.1 static host runtime");
  const lock = readLock();
  console.log(
    `pin: ${lock.release} ${lock.targetOs}/${lock.targetArch} ${lock.versionOutput}`
  );

  let acquired;
  try {
    acquired = await acquireWorkerd();
  } catch (err) {
    console.log(`failed          acquire-pinned-workerd`);
    console.log(`  ${err && err.stack ? err.stack : err}`);
    for (const name of CASE_NAMES) reporter.notRun(name, "workerd acquire failed");
    return reporter.summary().code || 1;
  }
  console.log(`binary: ${acquired.binPath}`);
  console.log(`artifact: ${acquired.gzPath}`);

  await reporter.test("lock-version-checksum", async () => {
    assert.equal(os.platform(), lock.targetOs, "target os");
    assert.equal(os.arch(), lock.targetArch, "target arch");
    assert.equal(sha256File(acquired.gzPath), lock.artifact.sha256, "artifact sha256");
    const verified = verifyBinary(acquired.binPath, lock);
    assert.equal(verified.sha256, lock.binary.sha256, "binary sha256");
    assert.equal(verified.sha256, lock.sha256, "lock sha256");
    assert.equal(verified.versionOutput, lock.versionOutput, "workerd --version");
    assert.equal(lock.versionOutput, "workerd 2026-08-26", "pinned version output");
    assert.isTrue(Array.isArray(lock.requiredProcessFlags), "required process flags present");
    assert.includes(lock.requiredProcessFlags.join(" "), "--experimental", "experimental flag pinned");
  });

  await reporter.test("checksum-mismatch-before-spawn", async () => {
    const badLock = JSON.parse(JSON.stringify(lock));
    badLock.sha256 = "0".repeat(64);
    badLock.binary = { ...badLock.binary, sha256: "0".repeat(64) };
    await assert.rejects(
      async () => verifyBinary(acquired.binPath, badLock),
      "binary checksum mismatch",
      "wrong lock hash must fail before exec"
    );

    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "g0-tamper-"));
    const tampered = path.join(scratch, "workerd");
    fs.copyFileSync(acquired.binPath, tampered);
    fs.chmodSync(tampered, 0o755);
    fs.appendFileSync(tampered, Buffer.from([0x00]));
    const proc = new WorkerdProcess({
      binPath: tampered,
      lock,
      runDir: path.join(scratch, "run"),
    });
    const err = await assert.rejects(
      () => proc.start(),
      "binary checksum mismatch",
      "tampered binary must fail before serve"
    );
    assert.isTrue(!proc.spawnedServe, "serve child must not be spawned");
    assert.isTrue(proc.pid == null, "no serve pid after checksum failure");
    assert.isFalse(proc.isAlive(), "no live child after checksum failure");
    assert.includes(err.message, "checksum", "error names checksum");
    fs.rmSync(scratch, { recursive: true, force: true });
  });

  await reporter.test("config-parses-with-pinned-binary", async () => {
    const result = runWorkerdOnce(
      acquired.binPath,
      ["compile", path.join(WORKERD_DIR, "config.capnp"), "--config-only"],
      20000,
      { encoding: "buffer" }
    );
    assert.equal(result.status, 0, "compile status");
    assert.isTrue(Buffer.isBuffer(result.stdout) && result.stdout.length > 0, "compiled config bytes");
  });

  await reporter.test("invalid-config-nonzero", async () => {
    const invalidPath = path.join(WORKERD_DIR, "invalid.capnp");
    const compile = runWorkerdOnce(
      acquired.binPath,
      ["compile", invalidPath, "--config-only"],
      8000,
      { encoding: "buffer" }
    );
    assert.isTrue(compile.status !== 0 && compile.status != null, "invalid compile exits nonzero");
    const serve = runWorkerdOnce(acquired.binPath, ["serve", invalidPath, "--experimental"]);
    assert.isTrue(serve.status !== 0 && serve.status != null, "invalid serve exits nonzero");

    const proc = new WorkerdProcess({
      binPath: acquired.binPath,
      lock: acquired.lock,
      configPath: invalidPath,
    });
    await assert.rejects(
      () => proc.start(),
      "config compile/parse failed",
      "supervisor must fail closed on invalid config"
    );
    assert.isTrue(!proc.spawnedServe, "invalid config must not spawn serve");
    assert.isFalse(proc.isAlive(), "no live child after invalid config");
    await proc.cleanupSuccess();
  });

  await reporter.test("port-collision-fail-closed", async () => {
    const occupied = await occupyEphemeral();
    const proc = new WorkerdProcess({
      binPath: acquired.binPath,
      lock: acquired.lock,
      address: `127.0.0.1:${occupied.port}`,
    });
    try {
      const err = await assert.rejects(
        () => proc.start(),
        /exited before listen|already in use|Address already in use/i,
        "occupied port must fail closed"
      );
      assert.isFalse(proc.isAlive(), "workerd must not remain running on bind failure");
      assert.isTrue(proc.exit != null, "bind failure must be an observed child exit");
      assert.isTrue(proc.exit.code !== 0, "bind failure must be a nonzero child exit");
      assert.includes(err.message + proc.readLogs(), "already in use", "logs mention bind collision");
    } finally {
      await closeServer(occupied.server);
      if (proc.isAlive()) await proc.kill("SIGKILL");
      await proc.cleanupSuccess();
    }
  });

  await reporter.test("unwritable-data-dir-fail-closed", async () => {
    const proc = new WorkerdProcess({
      binPath: acquired.binPath,
      lock: acquired.lock,
    });
    proc.prepareDirs();
    fs.chmodSync(proc.dataDir, 0o500);
    try {
      const err = await assert.rejects(
        () => proc.start(),
        /exited before listen|Permission denied/i,
        "unwritable data dir must fail closed"
      );
      assert.isFalse(proc.isAlive(), "workerd must not remain running");
      assert.isTrue(proc.exit != null, "unwritable dir must be an observed child exit");
      const logs = err.message + "\n" + proc.readLogs();
      assert.includes(logs, "Permission denied", "logs mention permission denied");
    } finally {
      try {
        fs.chmodSync(proc.dataDir, 0o755);
      } catch {
        /* ignore */
      }
      if (proc.isAlive()) await proc.kill("SIGKILL");
      await proc.cleanupSuccess();
    }
  });

  await reporter.test("health-only-after-ready", async () => {
    const port = await freePort();
    const healthUrl = `http://127.0.0.1:${port}/health`;
    const before = await probeHealth(healthUrl);
    assert.isFalse(before.ok, "health must not succeed before workerd starts");
    assert.isFalse(before.reached && before.json?.ok, "health json must not be ok before start");

    await withWorkerd(
      acquired,
      async (proc) => {
        assert.isTrue(proc.listenAt != null, "control-fd listen recorded");
        assert.isTrue(proc.readyAt != null, "ready recorded");
        assert.isTrue(proc.readyAt >= proc.listenAt, "ready is after listen");
        assert.equal(proc.port, port, "listen port matches assigned free port");
        const listenSockets = proc.listenEvents.filter((e) => e.event === "listen").map((e) => e.socket);
        assert.deepEqual(listenSockets, ["http"], "only the public http socket is advertised");
        assert.isTrue(proc.isAlive(), "pid is live after ready");
        process.kill(proc.pid, 0);
        const health = await proc.client.health();
        assert.okStatus(health, "/health after ready");
        assert.equal(health.json.ok, true, "health ok");
        assert.equal(health.json.service, "ingress", "health service");
      },
      { address: `127.0.0.1:${port}` }
    );
  });

  await reporter.test("default-entrypoint", async () => {
    await withWorkerd(acquired, async (proc) => {
      const res = await proc.client.request("/echo");
      assert.okStatus(res, "GET /echo");
      assert.equal(res.json.ok, true, "echo ok");
      assert.equal(res.json.service, "echo", "echo service");
      assert.equal(res.json.entrypoint, "default", "default entrypoint");
    });
  });

  await reporter.test("named-entrypoint", async () => {
    await withWorkerd(acquired, async (proc) => {
      const res = await proc.client.request("/echo/named");
      assert.okStatus(res, "GET /echo/named");
      assert.equal(res.json.ok, true, "named ok");
      assert.equal(res.json.service, "echo", "echo service");
      assert.equal(res.json.entrypoint, "named", "named entrypoint");
    });
  });

  await reporter.test("internal-paths-not-public", async () => {
    await withWorkerd(acquired, async (proc) => {
      for (const pathname of INTERNAL_PATHS) {
        const res = await proc.client.request(pathname);
        assert.equal(res.status, 404, `${pathname} status`);
        assert.isFalse(res.ok, `${pathname} must not be ok`);
        assert.equal(res.json?.ok, false, `${pathname} json.ok`);
        assert.equal(res.json?.errorCode, "NOT_PUBLIC", `${pathname} errorCode`);
        assert.isFalse(res.json?.service === "echo", `${pathname} must not be echo`);
      }
      const still = await proc.client.request("/echo");
      assert.okStatus(still, "echo still works after blocked paths");
    });
  });

  await reporter.test("handler-exception-contained", async () => {
    await withWorkerd(acquired, async (proc) => {
      const thrown = await proc.client.request("/echo/throw");
      assert.equal(thrown.status, 500, "echo throw status");
      assert.isFalse(thrown.ok, "echo throw is not ok");
      const ingressThrown = await proc.client.request("/g0/throw");
      assert.equal(ingressThrown.status, 500, "ingress throw status");
      assert.equal(ingressThrown.json?.errorCode, "INTERNAL", "ingress throw errorCode");
      assert.isTrue(proc.isAlive(), "workerd still alive after handler throw");
      const health = await proc.client.health();
      assert.okStatus(health, "health after throw");
      const echo = await proc.client.request("/echo");
      assert.okStatus(echo, "echo after throw");
      assert.equal(echo.json.entrypoint, "default", "default echo after throw");
    });
  });

  await reporter.test("sigterm-exits", async () => {
    const proc = new WorkerdProcess({ binPath: acquired.binPath, lock: acquired.lock });
    try {
      await proc.start();
      const pid = proc.pid;
      assert.isTrue(proc.isAlive(), "alive before SIGTERM");
      const exit = await proc.stop("SIGTERM");
      assert.isTrue(exit != null, "exit observed");
      assert.isFalse(proc.isAlive(), "not alive after SIGTERM");
      try {
        process.kill(pid, 0);
        throw new Error(`pid ${pid} still exists after SIGTERM`);
      } catch (err) {
        if (err.code !== "ESRCH") throw err;
      }
      await proc.cleanupSuccess();
    } catch (err) {
      proc.retainFailed(err);
      try {
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
      throw err;
    }
  });

  await reporter.test("sigkill-observed", async () => {
    const proc = new WorkerdProcess({ binPath: acquired.binPath, lock: acquired.lock });
    try {
      await proc.start();
      const pid = proc.pid;
      const exit = await proc.kill("SIGKILL");
      assert.isTrue(exit != null, "exit observed");
      assert.equal(exit.signal, "SIGKILL", "exit signal");
      assert.isFalse(proc.isAlive(), "not alive after SIGKILL");
      try {
        process.kill(pid, 0);
        throw new Error(`pid ${pid} still exists after SIGKILL`);
      } catch (err) {
        if (err.code !== "ESRCH") throw err;
      }
      await proc.cleanupSuccess();
    } catch (err) {
      proc.retainFailed(err);
      try {
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
      throw err;
    }
  });

  await reporter.test("restart-new-pid", async () => {
    const proc = new WorkerdProcess({ binPath: acquired.binPath, lock: acquired.lock });
    try {
      await proc.start();
      const first = proc.pid;
      assert.isTrue(proc.isAlive(), "first pid alive");
      const health1 = await proc.client.health();
      assert.okStatus(health1, "health before restart");
      const { previousPid, pid } = await proc.restart("SIGKILL");
      assert.equal(previousPid, first, "restart reports previous pid");
      assert.isTrue(pid != null && pid !== first, "new pid assigned");
      assert.isTrue(proc.isAlive(), "new pid alive");
      try {
        process.kill(first, 0);
        throw new Error(`old pid ${first} still exists after restart`);
      } catch (err) {
        if (err.code !== "ESRCH") throw err;
      }
      const health2 = await proc.client.health();
      assert.okStatus(health2, "health after restart");
      await proc.stop("SIGTERM");
      await proc.cleanupSuccess();
    } catch (err) {
      proc.retainFailed(err);
      try {
        await proc.kill("SIGKILL");
      } catch {
        /* ignore */
      }
      throw err;
    }
  });

  await reporter.test("harness-exit-reaps-child", async () => {
    const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "g0-orphan-"));
    const pidPath = path.join(scratch, "workerd.pid");
    const runDir = path.join(scratch, "run");
    ensureDir(runDir);
    const helper = spawn(process.execPath, [path.join(G0_ROOT, "tests", "orphan-helper.js"), pidPath, runDir], {
      cwd: G0_ROOT,
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    helper.stdout.on("data", (buf) => {
      stdout += buf.toString("utf8");
    });
    helper.stderr.on("data", (buf) => {
      stderr += buf.toString("utf8");
    });
    const helperExit = await new Promise((resolve) => helper.on("exit", (code, signal) => resolve({ code, signal })));
    assert.equal(helperExit.code, 0, `orphan helper exit (${stderr || stdout})`);
    assert.isTrue(fs.existsSync(pidPath), "helper wrote workerd pid");
    const childPid = Number(fs.readFileSync(pidPath, "utf8").trim());
    assert.isTrue(Number.isInteger(childPid) && childPid > 0, "workerd pid");
    try {
      process.kill(childPid, 0);
      try {
        process.kill(childPid, "SIGKILL");
      } catch {
        /* ignore */
      }
      throw new Error(`workerd pid ${childPid} still alive after harness exit`);
    } catch (err) {
      if (err.code !== "ESRCH") throw err;
    }
    fs.rmSync(scratch, { recursive: true, force: true });
  });

  await reporter.test("no-leaked-workerd-child", async () => {
    assert.equal(liveCount(), 0, "supervisor live child count");
  });

  const summary = reporter.summary();
  if (summary.failed === 0 && summary.notRun === 0) {
    console.log("G0.0/G0.1: PASS");
  } else {
    console.log("G0.0/G0.1: FAIL");
  }
  return summary.code;
}

module.exports = { run };

if (require.main === module) {
  run()
    .then((code) => process.exit(code))
    .catch((err) => {
      console.error(err);
      process.exit(1);
    });
}
