"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { pipeline } = require("node:stream/promises");
const zlib = require("node:zlib");
const { spawnSync } = require("node:child_process");

const G0_ROOT = path.resolve(__dirname, "..");
const LOCK_PATH = path.join(G0_ROOT, "workerd.lock");
const CACHE_ROOT = path.resolve(G0_ROOT, "../.temp/runtime-cache");

function readLock() {
  return JSON.parse(fs.readFileSync(LOCK_PATH, "utf8"));
}

function sha256File(filePath) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(filePath));
  return hash.digest("hex");
}

function assertPlatform(lock) {
  if (os.platform() !== lock.targetOs || os.arch() !== lock.targetArch) {
    throw new Error(
      `workerd lock is pinned to ${lock.targetOs}/${lock.targetArch}; host is ${os.platform()}/${os.arch()}`
    );
  }
}

function download(url, dest) {
  return new Promise((resolve, reject) => {
    const get = (current) => {
      https
        .get(
          current,
          {
            headers: { "User-Agent": "open-compute-g0-spike/1.0" },
          },
          (res) => {
            if (res.statusCode >= 300 && res.statusCode < 400 && res.headers.location) {
              const next = new URL(res.headers.location, current).href;
              res.resume();
              get(next);
              return;
            }
            if (res.statusCode !== 200) {
              reject(new Error(`download failed: ${res.statusCode} ${current}`));
              return;
            }
            const out = fs.createWriteStream(dest);
            res.pipe(out);
            out.on("finish", () => out.close(resolve));
            out.on("error", reject);
          }
        )
        .on("error", reject);
    };
    get(url);
  });
}

async function acquireWorkerd(options = {}) {
  const lock = options.lock || readLock();
  assertPlatform(lock);
  const versionDir = path.join(CACHE_ROOT, lock.release);
  fs.mkdirSync(versionDir, { recursive: true });
  const gzPath = path.join(versionDir, lock.artifact.name);
  const binPath = path.join(versionDir, lock.binary?.name || "workerd");
  const expectedArtifact = lock.artifact.sha256;
  const expectedBinary = lock.binary?.sha256 || lock.sha256;

  const artifactOk = fs.existsSync(gzPath) && sha256File(gzPath) === expectedArtifact;
  if (!artifactOk) {
    if (fs.existsSync(gzPath)) fs.rmSync(gzPath, { force: true });
    const partial = `${gzPath}.partial`;
    if (fs.existsSync(partial)) fs.rmSync(partial, { force: true });
    await download(lock.artifact.url, partial);
    const gzHash = sha256File(partial);
    if (gzHash !== expectedArtifact) {
      fs.rmSync(partial, { force: true });
      throw new Error(
        `artifact checksum mismatch for ${lock.artifact.name}: expected ${expectedArtifact} got ${gzHash}`
      );
    }
    fs.renameSync(partial, gzPath);
  }

  const binaryOk = fs.existsSync(binPath) && sha256File(binPath) === expectedBinary;
  if (!binaryOk) {
    if (fs.existsSync(binPath)) fs.rmSync(binPath, { force: true });
    await pipeline(fs.createReadStream(gzPath), zlib.createGunzip(), fs.createWriteStream(binPath));
    fs.chmodSync(binPath, 0o755);
  }

  verifyBinary(binPath, lock);
  return { lock, binPath, gzPath, cacheDir: versionDir };
}

function verifyBinary(binPath, lock = readLock()) {
  if (!fs.existsSync(binPath)) {
    throw new Error("workerd binary missing; cannot start");
  }
  const actual = sha256File(binPath);
  const expected = lock.binary?.sha256 || lock.sha256;
  if (actual !== expected) {
    throw new Error(`binary checksum mismatch: expected ${expected} got ${actual}`);
  }
  const version = spawnSync(binPath, ["--version"], { encoding: "utf8" });
  if (version.status !== 0) {
    throw new Error(`workerd --version failed: ${version.stderr || version.stdout}`);
  }
  const output = (version.stdout || "").trim();
  if (output !== lock.versionOutput) {
    throw new Error(`workerd --version mismatch: expected ${lock.versionOutput} got ${output}`);
  }
  return { sha256: actual, versionOutput: output };
}

module.exports = {
  G0_ROOT,
  LOCK_PATH,
  CACHE_ROOT,
  readLock,
  sha256File,
  acquireWorkerd,
  verifyBinary,
  assertPlatform,
};
