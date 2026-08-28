"use strict";

const fs = require("node:fs");
const net = require("node:net");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const { G0_ROOT, verifyBinary } = require("./runtime");
const { G0Client, waitForHealth } = require("./http");

const RUN_ROOT = path.resolve(G0_ROOT, "../.temp/g0-run");
const WORKERD_DIR = path.join(G0_ROOT, "workerd");
const FAILED_ROOT = path.join(RUN_ROOT, "failed");

const liveProcesses = new Set();
let exitHooksInstalled = false;

function installExitHooks() {
  if (exitHooksInstalled) return;
  exitHooksInstalled = true;
  const reap = () => {
    for (const proc of liveProcesses) {
      try {
        proc.reapSync("SIGKILL");
      } catch {
        /* already gone */
      }
    }
  };
  process.on("exit", reap);
  for (const signal of ["SIGINT", "SIGTERM", "SIGHUP"]) {
    process.on(signal, () => {
      reap();
      process.exit(signal === "SIGINT" ? 130 : 143);
    });
  }
}

function nowId() {
  return `${Date.now()}-${Math.random().toString(16).slice(2, 8)}`;
}

function ensureDir(dir) {
  fs.mkdirSync(dir, { recursive: true });
  return dir;
}

function freePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen({ host: "127.0.0.1", port: 0, exclusive: true }, () => {
      const { port } = server.address();
      server.close((err) => (err ? reject(err) : resolve(port)));
    });
  });
}

function occupyPort(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen({ host: "127.0.0.1", port, exclusive: true }, () => resolve(server));
  });
}

function occupyEphemeral() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.on("error", reject);
    server.listen({ host: "127.0.0.1", port: 0, exclusive: true }, () => {
      resolve({ server, port: server.address().port });
    });
  });
}

function closeServer(server) {
  return new Promise((resolve) => {
    if (!server) return resolve();
    server.close(() => resolve());
  });
}

class WorkerdProcess {
  constructor(options) {
    this.binPath = options.binPath;
    this.lock = options.lock;
    this.runId = options.runId || nowId();
    this.runDir = options.runDir || path.join(RUN_ROOT, this.runId);
    this.dataDir = options.dataDir || path.join(this.runDir, "data", "do");
    this.logPath = path.join(this.runDir, "workerd.stderr.log");
    this.stdoutPath = path.join(this.runDir, "workerd.stdout.log");
    this.compiledPath = path.join(this.runDir, "config.bin");
    this.argvPath = path.join(this.runDir, "argv.json");
    this.pidPath = path.join(this.runDir, "workerd.pid");
    this.controlLogPath = path.join(this.runDir, "control.log");
    this.configPath = options.configPath || path.join(WORKERD_DIR, "config.capnp");
    this.fixturesDir = options.fixturesDir || path.join(G0_ROOT, "fixtures");
    this.address = options.address || null;
    this.child = null;
    this.pid = null;
    this.port = null;
    this.baseUrl = null;
    this.client = null;
    this.exit = null;
    this.exitPromise = null;
    this.startedAt = null;
    this.listenAt = null;
    this.readyAt = null;
    this.controlChunks = "";
    this.listenEvents = [];
    this.spawnedServe = false;
    this.retainOnFailure = options.retainOnFailure !== false;
  }

  prepareDirs() {
    ensureDir(this.runDir);
    ensureDir(this.dataDir);
    ensureDir(path.join(this.runDir, "tmp"));
  }

  compileConfig() {
    const result = spawnSync(this.binPath, ["compile", this.configPath, "--config-only"], {
      cwd: WORKERD_DIR,
      encoding: "buffer",
      maxBuffer: 32 * 1024 * 1024,
      timeout: 20000,
    });
    const stderr = result.stderr ? result.stderr.toString("utf8") : "";
    fs.writeFileSync(path.join(this.runDir, "compile.stderr.log"), stderr);
    if (result.status !== 0 || !result.stdout || result.stdout.length === 0) {
      const err = new Error(
        `config compile/parse failed: status=${result.status} signal=${result.signal} stderr=${stderr.slice(-2000)}`
      );
      err.compileResult = { status: result.status, signal: result.signal, stderr };
      throw err;
    }
    fs.writeFileSync(this.compiledPath, result.stdout);
    return result;
  }

  async start() {
    if (this.child && !this.exit) throw new Error("workerd already started");
    this.child = null;
    this.exit = null;
    this.pid = null;
    this.baseUrl = null;
    this.client = null;
    this.controlChunks = "";
    this.listenEvents = [];
    this.listenAt = null;
    this.readyAt = null;
    this.spawnedServe = false;

    verifyBinary(this.binPath, this.lock);
    this.prepareDirs();
    this.compileConfig();

    if (!this.address) {
      const port = await freePort();
      this.address = `127.0.0.1:${port}`;
      this.port = port;
    } else if (this.port == null) {
      const match = /:(\d+)$/.exec(this.address);
      if (match) this.port = Number(match[1]);
    }

    const args = [
      "serve",
      "--binary",
      "-",
      ...this.lock.requiredProcessFlags,
      `--directory-path=g0-do-disk=${this.dataDir}`,
      `--directory-path=g0-fixtures=${this.fixturesDir}`,
      "--control-fd=3",
      "--verbose",
      `--socket-addr=http=${this.address}`,
    ];
    fs.writeFileSync(
      this.argvPath,
      JSON.stringify(
        {
          bin: path.basename(this.binPath),
          args,
          address: this.address,
          config: path.basename(this.configPath),
        },
        null,
        2
      )
    );

    const stdinFd = fs.openSync(this.compiledPath, "r");
    const stdoutFd = fs.openSync(this.stdoutPath, "w");
    const stderrFd = fs.openSync(this.logPath, "w");
    this.startedAt = Date.now();
    try {
      this.child = spawn(this.binPath, args, {
        cwd: WORKERD_DIR,
        stdio: [stdinFd, stdoutFd, stderrFd, "pipe"],
        detached: true,
        env: { ...process.env, TMPDIR: path.join(this.runDir, "tmp") },
      });
    } finally {
      fs.closeSync(stdinFd);
      fs.closeSync(stdoutFd);
      fs.closeSync(stderrFd);
    }

    this.spawnedServe = true;
    this.pid = this.child.pid;
    fs.writeFileSync(this.pidPath, String(this.pid));
    liveProcesses.add(this);
    installExitHooks();

    this.exitPromise = new Promise((resolve) => {
      this.child.on("exit", (code, signal) => {
        this.exit = { code, signal, at: Date.now() };
        liveProcesses.delete(this);
        resolve(this.exit);
      });
    });
    this.child.on("error", (err) => {
      this.spawnError = err;
    });

    try {
      await this.#waitListen(this.child.stdio[3]);
      this.baseUrl = `http://127.0.0.1:${this.port}`;
      this.client = new G0Client(this.baseUrl);
      const abort = new AbortController();
      const onExit = () => abort.abort();
      this.child.once("exit", onExit);
      try {
        await waitForHealth(this.client, { timeoutMs: 15000, signal: abort.signal });
      } finally {
        this.child.off("exit", onExit);
      }
      if (!this.listenAt) {
        throw new Error("refusing to mark ready: control-fd did not report http listen");
      }
      this.readyAt = Date.now();
      return this;
    } catch (err) {
      if (this.child && !this.exit) {
        this.reapSync("SIGKILL");
        if (this.exitPromise) await this.exitPromise;
      }
      throw err;
    }
  }

  #waitListen(control) {
    return new Promise((resolve, reject) => {
      if (!control) {
        reject(new Error("workerd control fd missing"));
        return;
      }
      const timeout = setTimeout(() => {
        cleanup();
        reject(
          new Error(`timed out waiting for listen event; logs=${this.readLogs().slice(-2000)}`)
        );
      }, 20000);
      const onExit = (code, signal) => {
        cleanup();
        reject(
          new Error(
            `workerd exited before listen: code=${code} signal=${signal} logs=${this.readLogs().slice(-2000)}`
          )
        );
      };
      const onError = (err) => {
        cleanup();
        reject(new Error(`workerd spawn error before listen: ${err.message}`));
      };
      const onData = (buf) => {
        this.controlChunks += buf.toString("utf8");
        fs.writeFileSync(this.controlLogPath, this.controlChunks);
        const lines = this.controlChunks.split("\n");
        this.controlChunks = lines.pop() || "";
        for (const line of lines) {
          if (!line.trim()) continue;
          let msg;
          try {
            msg = JSON.parse(line);
          } catch {
            continue;
          }
          this.listenEvents.push(msg);
          if (msg.event === "listen" && msg.socket === "http") {
            this.port = msg.port;
            this.listenAt = Date.now();
            cleanup();
            resolve(msg);
          }
        }
      };
      const cleanup = () => {
        clearTimeout(timeout);
        control.off("data", onData);
        this.child.off("exit", onExit);
        this.child.off("error", onError);
      };
      control.on("data", onData);
      this.child.once("exit", onExit);
      this.child.once("error", onError);
    });
  }

  readLogs() {
    try {
      return fs.readFileSync(this.logPath, "utf8");
    } catch {
      return "";
    }
  }

  isAlive() {
    if (!this.pid || this.exit) return false;
    try {
      process.kill(this.pid, 0);
      return true;
    } catch {
      return false;
    }
  }

  reapSync(signal = "SIGKILL") {
    if (!this.pid) return;
    try {
      process.kill(this.pid, signal);
    } catch (err) {
      if (err.code !== "ESRCH") throw err;
    }
  }

  async stop(signal = "SIGTERM") {
    if (!this.child || this.exit) return this.exit;
    this.reapSync(signal);
    let timer;
    const timeout = new Promise((resolve) => {
      timer = setTimeout(resolve, 5000);
    });
    try {
      const winner = await Promise.race([
        this.exitPromise.then(() => "exit"),
        timeout.then(() => "timeout"),
      ]);
      if (winner === "timeout" && !this.exit) {
        this.reapSync("SIGKILL");
        await this.exitPromise;
      }
      return this.exit;
    } finally {
      clearTimeout(timer);
      this.child = null;
      liveProcesses.delete(this);
    }
  }

  async kill(signal = "SIGKILL") {
    return this.stop(signal);
  }

  async restart(signal = "SIGKILL") {
    const previousPid = this.pid;
    await this.stop(signal);
    await this.start();
    return { previousPid, pid: this.pid };
  }

  ownsRunDir() {
    const runDir = path.resolve(this.runDir);
    const root = path.resolve(RUN_ROOT);
    const failed = path.resolve(FAILED_ROOT);
    return runDir.startsWith(root + path.sep) && !runDir.startsWith(failed + path.sep);
  }

  async cleanupSuccess() {
    if (!this.ownsRunDir()) return;
    if (!fs.existsSync(this.runDir)) return;
    try {
      if (fs.existsSync(this.dataDir)) fs.chmodSync(this.dataDir, 0o755);
    } catch {
      /* ignore */
    }
    fs.rmSync(this.runDir, { recursive: true, force: true });
  }

  retainFailed(reason) {
    const failedDir = ensureDir(path.join(FAILED_ROOT, this.runId));
    const meta = {
      reason: String(reason && reason.message ? reason.message : reason),
      pid: this.pid,
      port: this.port,
      exit: this.exit,
      host: os.hostname(),
      runId: this.runId,
    };
    try {
      ensureDir(this.runDir);
      fs.writeFileSync(path.join(this.runDir, "failure.json"), JSON.stringify(meta, null, 2));
    } catch {
      fs.writeFileSync(path.join(failedDir, "failure.json"), JSON.stringify(meta, null, 2));
    }
    if (path.resolve(this.runDir) !== path.resolve(failedDir) && fs.existsSync(this.runDir)) {
      fs.cpSync(this.runDir, failedDir, { recursive: true, force: true });
    }
    return failedDir;
  }
}

async function spawnWorkerd(options) {
  const proc = new WorkerdProcess(options);
  await proc.start();
  return proc;
}

function runWorkerdOnce(binPath, args, timeoutMs = 8000, options = {}) {
  return spawnSync(binPath, args, {
    cwd: WORKERD_DIR,
    encoding: options.encoding ?? "utf8",
    timeout: timeoutMs,
    maxBuffer: options.maxBuffer ?? 32 * 1024 * 1024,
  });
}

function liveCount() {
  let n = 0;
  for (const proc of liveProcesses) {
    if (proc.isAlive()) n += 1;
  }
  return n;
}

module.exports = {
  RUN_ROOT,
  FAILED_ROOT,
  WORKERD_DIR,
  WorkerdProcess,
  spawnWorkerd,
  runWorkerdOnce,
  freePort,
  occupyPort,
  occupyEphemeral,
  closeServer,
  nowId,
  ensureDir,
  liveCount,
};
