import { Buffer } from "node:buffer";
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync } from "node:fs";
import { connect as nodeConnect } from "node:net";
import { join } from "node:path";
import process from "node:process";
import { connect as socketConnect } from "cloudflare:sockets";

const DENIED = /not allowed|disallowed|denied|refused by|private network|network address|proxy request failed/i;

async function socketAttempt(address, options) {
  let socket;
  try {
    socket = socketConnect(address, options);
    await Promise.race([
      socket.opened,
      scheduler.wait(1000).then(() => { throw new Error("probe timeout"); }),
    ]);
    await socket.close();
    return { opened: true, denied: false, name: null };
  } catch (error) {
    try { await socket?.close(); } catch {}
    const message = String(error && error.message || error);
    return {
      opened: false,
      denied: DENIED.test(message),
      limited: /subrequest|limit/i.test(message),
      name: error && error.name || null,
    };
  }
}

function nodeAttempt(host, port) {
  return new Promise(resolve => {
    let settled = false;
    const socket = nodeConnect({ host, port });
    const finish = result => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      socket.destroy();
      resolve(result);
    };
    const timer = setTimeout(
      () => finish({ opened: false, denied: false, timeout: true, code: null }),
      1000,
    );
    socket.once("connect", () => finish({ opened: true, denied: false, timeout: false, code: null }));
    socket.once("error", error => {
      const message = String(error && error.message || error);
      finish({
        opened: false,
        denied: DENIED.test(message),
        timeout: false,
        code: error && error.code || null,
      });
    });
  });
}

async function rawTcpNegative() {
  const networkTargets = [
    "127.0.0.1:1",
    "10.0.0.1:1",
    "169.254.169.254:80",
    "[::1]:1",
    "[::ffff:127.0.0.1]:1",
    "localhost:1",
  ];
  const malformedTargets = [
    "127.0.0.1",
    ":80",
    "bad host:80",
    "unix:/tmp/open-compute.sock",
    { hostname: "127.0.0.1", port: 70000 },
  ];
  return {
    sockets: await Promise.all(networkTargets.map(target => socketAttempt(target))),
    malformed: await Promise.all(malformedTargets.map(target => socketAttempt(target))),
    invalidTransport: await socketAttempt("example.com:443", {
      secureTransport: "invalid",
      allowHalfOpen: false,
    }),
    node: await Promise.all([
      nodeAttempt("127.0.0.1", 1),
      nodeAttempt("10.0.0.1", 1),
      nodeAttempt("169.254.169.254", 80),
      nodeAttempt("::1", 1),
      nodeAttempt("::ffff:127.0.0.1", 1),
      nodeAttempt("localhost", 1),
    ]),
  };
}

function hostFsProbe(path, expectedDigest) {
  const result = { exists: false, read: false, digestMatched: false };
  if (typeof path !== "string" || typeof expectedDigest !== "string") return result;
  try {
    result.exists = existsSync(path) === true;
  } catch {
    result.exists = false;
  }
  try {
    const digest = createHash("sha256").update(readFileSync(path)).digest("hex");
    result.read = true;
    result.digestMatched = digest === expectedDigest;
  } catch {
    result.read = false;
    result.digestMatched = false;
  }
  return result;
}

export default {
  async fetch(request, env) {
    const action = await request.text();
    if (action === "raw-tcp-negative") return Response.json(await rawTcpNegative());
    let childProcess = { threw: false };
    try {
      spawn("true");
    } catch (error) {
      childProcess = {
        threw: true,
        code: error && error.code,
      };
    }

    let sockets = { imported: false, hasConnect: false };
    try {
      const mod = await import("cloudflare:sockets");
      sockets = {
        imported: true,
        hasConnect: typeof mod.connect === "function",
      };
    } catch {
      sockets = { imported: false, hasConnect: false };
    }

    return Response.json({
      buffer: Buffer.from("node-compat").toString(),
      digest: createHash("sha256").update("open-compute").digest("hex"),
      path: join("a", "b"),
      globalBuffer: typeof Buffer === "function",
      envKeys: Object.keys(env).sort(),
      greeting: env.GREETING,
      hasToken: typeof env.TOKEN === "string" && env.TOKEN.length > 0,
      processEnvKeys: Object.keys(process.env).sort(),
      processGreeting: process.env.GREETING,
      processToken: process.env.TOKEN,
      processPlatformSecret: process.env.OPEN_COMPUTE_NODE_ISOLATION_SECRET,
      processPath: process.env.PATH,
      processHome: process.env.HOME,
      hostFs: hostFsProbe(env.HOST_PROBE_PATH, env.HOST_PROBE_DIGEST),
      childProcess,
      sockets,
    });
  },
};
