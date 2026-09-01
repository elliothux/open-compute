import { connect as nodeConnect } from "node:net";
import { connect as tlsConnect } from "node:tls";
import { connect as socketConnect } from "cloudflare:sockets";
import { WorkerEntrypoint } from "cloudflare:workers";

const DENIED = /not allowed|disallowed|denied|refused by|private network|network address|proxy request failed/i;
const encoder = new TextEncoder();

async function deadline(promise, label, milliseconds = 5000) {
  let timeout;
  try {
    return await Promise.race([
      promise,
      new Promise((_resolve, reject) => {
        timeout = setTimeout(() => reject(new Error(`${label} timed out`)), milliseconds);
      }),
    ]);
  } finally {
    clearTimeout(timeout);
  }
}

function authority(hostname, port) {
  return hostname.includes(":") ? `[${hostname}]:${port}` : `${hostname}:${port}`;
}

function payload(size = 192 * 1024) {
  return Uint8Array.from({ length: size }, (_value, index) => index % 251);
}

async function readAndMatch(readable, expected) {
  const reader = readable.getReader();
  let offset = 0;
  let chunks = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      chunks++;
      for (const value of result.value) {
        if (offset >= expected.byteLength || value !== expected[offset]) {
          throw new Error("fixture echo mismatch");
        }
        offset++;
      }
    }
  } finally {
    reader.releaseLock();
  }
  if (offset !== expected.byteLength) throw new Error("fixture echo was truncated");
  return { bytes: offset, chunks };
}

async function socketEcho(address, options = { allowHalfOpen: true, secureTransport: "off" }) {
  const expected = payload();
  const socket = socketConnect(address, options);
  const opened = await deadline(socket.opened, "socket open");
  const writer = socket.writable.getWriter();
  const initialDesiredSize = writer.desiredSize;
  try {
    await writer.write(encoder.encode(`ECHO ${expected.byteLength}\n`));
    for (let offset = 0; offset < expected.byteLength; offset += 32 * 1024) {
      await writer.write(expected.subarray(offset, offset + 32 * 1024));
    }
    await writer.close();
  } finally {
    writer.releaseLock();
  }
  const received = await deadline(readAndMatch(socket.readable, expected), "socket echo read");
  await socket.close();
  await deadline(socket.closed, "socket close");
  await socket.close();
  return {
    ...received,
    initialDesiredSize,
    localAddress: opened.localAddress ?? null,
    remoteAddress: opened.remoteAddress ?? null,
  };
}

async function halfOpen(address, allowHalfOpen) {
  const socket = socketConnect(address, { allowHalfOpen, secureTransport: "off" });
  await deadline(socket.opened, "half-open socket open");
  const writer = socket.writable.getWriter();
  const reader = socket.readable.getReader();
  let marker = "";
  try {
    await writer.write(encoder.encode("HALF\n"));
    while (true) {
      const result = await deadline(reader.read(), "half-open peer EOF");
      if (result.done) break;
      marker += new TextDecoder().decode(result.value);
    }
  } finally {
    reader.releaseLock();
  }
  let writeAfterEof = true;
  try {
    await writer.write(encoder.encode("after-peer-eof"));
  } catch {
    writeAfterEof = false;
  }
  if (writeAfterEof) await writer.close();
  writer.releaseLock();
  if (allowHalfOpen) {
    await deadline(socket.close(), "half-open explicit close");
  } else {
    try { await socket.close(); } catch {}
  }
  let closeError = false;
  await deadline(socket.closed.catch(() => { closeError = true; }), "half-open socket close");
  return { marker, writeAfterEof, closeError };
}

async function deniedSocket(address) {
  let socket;
  try {
    socket = socketConnect(address, { allowHalfOpen: false, secureTransport: "off" });
    await deadline(socket.opened, "private socket rejection", 1500);
    await socket.close();
    return { opened: true, denied: false };
  } catch (error) {
    try { await socket?.close(); } catch {}
    return { opened: false, denied: DENIED.test(String(error && error.message || error)) };
  }
}

async function cloudflareTlsFailure(address, mode, expectedServerHostname) {
  let initial;
  let upgraded;
  let endpointReachable = false;
  let oldSocketNeutered = mode !== "starttls";
  try {
    if (mode === "on") {
      const probe = socketConnect(address, {
        allowHalfOpen: false,
        secureTransport: "off",
      });
      await deadline(probe.opened, "TLS endpoint reachability");
      endpointReachable = true;
      try { await deadline(probe.close(), "TLS endpoint probe cleanup", 1000); } catch {}
    }
    initial = socketConnect(address, {
      allowHalfOpen: false,
      secureTransport: mode,
    });
    if (mode === "starttls") {
      await deadline(initial.opened, "starttls TCP open");
      endpointReachable = true;
      upgraded = initial.startTls({ expectedServerHostname });
      let oldReader;
      try {
        oldReader = initial.readable.getReader();
        const result = await deadline(oldReader.read(), "starttls old socket read", 1000);
        oldSocketNeutered = result.done === true;
      } catch {
        oldSocketNeutered = true;
      } finally {
        try { oldReader?.releaseLock(); } catch { oldSocketNeutered = true; }
      }
    } else {
      upgraded = initial;
    }
    await deadline(upgraded.opened, "TLS certificate rejection");
    await upgraded.close();
    return {
      certificateRejected: false,
      initialSecureTransport: initial.secureTransport,
      initialUpgraded: initial.upgraded,
      oldSocketNeutered,
    };
  } catch (error) {
    const message = String(error && error.message || error);
    try {
      if (upgraded) await deadline(upgraded.close(), "TLS upgraded socket cleanup", 1000);
    } catch {}
    try {
      if (initial) await deadline(initial.close(), "TLS initial socket cleanup", 1000);
    } catch {}
    return {
      certificateRejected: endpointReachable && (
        /certificate|unknown issuer|unknown ca|self[- ]signed/i.test(message)
        || /proxy request failed/i.test(message)
      ),
      initialSecureTransport: initial?.secureTransport ?? null,
      initialUpgraded: initial?.upgraded ?? null,
      oldSocketNeutered,
      error: message,
    };
  }
}

function nodeEcho({ host, port }) {
  const expected = payload();
  return deadline(new Promise((resolve, reject) => {
    const socket = nodeConnect({ host, port, allowHalfOpen: true });
    let offset = 0;
    let chunks = 0;
    let settled = false;
    const fail = error => {
      if (settled) return;
      settled = true;
      socket.destroy();
      reject(error);
    };
    socket.setTimeout(5000);
    socket.once("timeout", () => fail(new Error("node echo timed out")));
    socket.once("error", fail);
    socket.on("data", chunk => {
      chunks++;
      for (const value of chunk) {
        if (offset >= expected.byteLength || value !== expected[offset]) {
          fail(new Error("node fixture echo mismatch"));
          return;
        }
        offset++;
      }
    });
    socket.once("end", () => {
      if (settled) return;
      if (offset !== expected.byteLength) {
        fail(new Error("node fixture echo was truncated"));
        return;
      }
      settled = true;
      socket.destroy();
      resolve({
        bytes: offset,
        chunks,
        destroyed: socket.destroyed === true,
      });
    });
    socket.once("connect", () => {
      socket.write(`ECHO ${expected.byteLength}\n`);
      for (let offset = 0; offset < expected.byteLength; offset += 32 * 1024) {
        socket.write(expected.subarray(offset, offset + 32 * 1024));
      }
      socket.end();
    });
  }), "node:net echo");
}

function nodeTlsFailure(host, port, servername) {
  return deadline(new Promise((resolve, reject) => {
    const probe = nodeConnect({ host, port });
    probe.once("error", reject);
    probe.once("connect", () => {
      probe.destroy();
      const socket = tlsConnect({
        host,
        port,
        servername,
        rejectUnauthorized: true,
      });
      socket.once("secureConnect", () => {
        socket.destroy();
        resolve({ certificateRejected: false, errorEvent: false, destroyed: socket.destroyed });
      });
      socket.once("error", error => {
        const message = String(error && error.message || error);
        socket.destroy();
        resolve({
          certificateRejected: /certificate|unknown issuer|unknown ca|self[- ]signed|proxy request failed/i.test(message),
          errorEvent: true,
          destroyed: socket.destroyed,
          error: message,
        });
      });
    });
  }), "node:tls certificate rejection");
}

function nodeTimeout(host, port) {
  return deadline(new Promise((resolve, reject) => {
    const socket = nodeConnect({ host, port });
    socket.once("connect", () => {
      socket.write("STALL\n");
      socket.setTimeout(100);
    });
    socket.once("timeout", () => {
      socket.destroy();
      resolve({ timedOut: true, destroyed: socket.destroyed === true });
    });
    socket.once("error", reject);
  }), "node:net timeout");
}

function nodeDenied(host, port) {
  return deadline(new Promise(resolve => {
    const socket = nodeConnect({ host, port });
    socket.once("connect", () => {
      socket.destroy();
      resolve({ opened: true, denied: false });
    });
    socket.once("error", error => {
      socket.destroy();
      resolve({
        opened: false,
        denied: DENIED.test(String(error && error.message || error)),
      });
    });
  }), "node private rejection", 1500);
}

async function loopbackEcho(service, label) {
  const expected = payload(96 * 1024);
  const expectedAuthority = "loopback.invalid:7000";
  const socket = service.connect(expectedAuthority, {
    allowHalfOpen: true,
  });
  await deadline(socket.opened, `${label} socket open`);
  const receiving = readLoopbackReply(socket.readable, expected);
  const writer = socket.writable.getWriter();
  try {
    for (let offset = 0; offset < expected.byteLength; offset += 16 * 1024) {
      await writer.write(expected.subarray(offset, offset + 16 * 1024));
    }
    await writer.close();
  } finally {
    writer.releaseLock();
  }
  const received = await deadline(receiving, `${label} socket read`);
  await socket.close();
  await deadline(socket.closed, `${label} socket close`);
  return { ...received, expectedAuthority };
}

async function readLoopbackReply(readable, expected) {
  const reader = readable.getReader();
  const parts = [];
  let total = 0;
  let chunks = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      chunks++;
      total += result.value.byteLength;
      if (total > expected.byteLength + 4096) throw new Error("loopback reply exceeds fixture bound");
      parts.push(result.value);
    }
  } finally {
    reader.releaseLock();
  }
  const reply = new Uint8Array(total);
  let offset = 0;
  for (const part of parts) {
    reply.set(part, offset);
    offset += part.byteLength;
  }
  const separator = reply.indexOf(10);
  if (separator < 0) throw new Error("loopback reply is missing socket metadata");
  const info = JSON.parse(new TextDecoder().decode(reply.subarray(0, separator)));
  const echoed = reply.subarray(separator + 1);
  if (echoed.byteLength !== expected.byteLength) throw new Error("loopback echo was truncated");
  for (let index = 0; index < expected.byteLength; index++) {
    if (echoed[index] !== expected[index]) throw new Error("loopback echo mismatch");
  }
  return {
    bytes: echoed.byteLength,
    chunks,
    localAddress: info.localAddress ?? null,
    remoteAddress: info.remoteAddress ?? null,
  };
}

async function loopbackProbe(service, label) {
  try {
    return { ok: true, ...(await loopbackEcho(service, label)) };
  } catch (error) {
    return { ok: false, stage: label, error: String(error && error.message || error) };
  }
}

async function matrixProbe(stage, operation) {
  try {
    return await deadline(Promise.resolve().then(operation), stage, 10_000);
  } catch (error) {
    return { stage, error: String(error && error.message || error) };
  }
}

async function probeGroup(prefix, entries) {
  const results = [];
  for (let offset = 0; offset < entries.length; offset += 3) {
    results.push(...await Promise.all(entries.slice(offset, offset + 3).map(
      async ([name, operation]) => [
        name,
        await matrixProbe(`${prefix}.${name}`, operation),
      ],
    )));
  }
  return Object.fromEntries(results);
}

async function rawTcpMatrix(config) {
  const tcpPort = Number(config.tcpPort);
  const tlsPort = Number(config.tlsPort);
  const hostnameAddress = authority(config.hostname, tcpPort);
  const tlsAddress = authority(config.hostname, tlsPort);
  const [sockets, node] = await Promise.all([
    probeGroup("sockets", [
      ["ipv4", () => socketEcho({ hostname: config.ipv4Host, port: tcpPort }, {
        allowHalfOpen: true,
        secureTransport: "off",
        highWaterMark: 4096n,
      })],
      ["ipv6", () => socketEcho(authority(config.ipv6Host, tcpPort), {
        allowHalfOpen: true,
        secureTransport: "off",
      })],
      ["dns", () => socketEcho(hostnameAddress)],
      ["halfOpenFalse", () => halfOpen(hostnameAddress, false)],
      ["halfOpenTrue", () => halfOpen(hostnameAddress, true)],
      ["tlsOn", () => cloudflareTlsFailure(tlsAddress, "on", config.hostname)],
      ["startTls", () => cloudflareTlsFailure(tlsAddress, "starttls", config.hostname)],
      ["privateDns", () => deniedSocket(authority(config.privateHostname, tcpPort))],
      ["loopback", () => deniedSocket(authority("127.0.0.1", tcpPort))],
    ]),
    probeGroup("node", [
      ["net", () => nodeEcho({ host: config.hostname, port: tcpPort })],
      ["tls", () => nodeTlsFailure(config.ipv4Host, tlsPort, config.hostname)],
      ["timeout", () => nodeTimeout(config.hostname, tcpPort)],
      ["privateDns", () => nodeDenied(config.privateHostname, tcpPort)],
      ["loopback", () => nodeDenied("127.0.0.1", tcpPort)],
    ]),
  ]);
  return {
    sockets,
    node,
  };
}

async function eventSourceRawTcp(env, source) {
  const config = JSON.parse(env.RAW_TCP_CONFIG_JSON);
  const tcpPort = Number(config.tcpPort);
  const echo = await socketEcho(authority(config.hostname, tcpPort));
  const denied = await deniedSocket(authority(config.privateHostname, tcpPort));
  if (echo.bytes !== 192 * 1024 || !denied.denied) {
    throw new Error(`${source} raw TCP event-source policy mismatch`);
  }
}

async function boundedEchoHandler(socket) {
  const opened = await socket.opened;
  let total = 0;
  await socket.readable.pipeThrough(new TransformStream({
    start(controller) {
      controller.enqueue(encoder.encode(`${JSON.stringify({
        localAddress: opened.localAddress ?? null,
        remoteAddress: opened.remoteAddress ?? null,
      })}\n`));
    },
    transform(chunk, controller) {
      total += chunk.byteLength;
      if (total > 256 * 1024) throw new Error("loopback socket payload exceeds fixture bound");
      controller.enqueue(chunk);
    },
  })).pipeTo(socket.writable);
}

export class SocketService extends WorkerEntrypoint {
  connect(socket) {
    return boundedEchoHandler(socket);
  }
}

export default {
  async fetch(_request, env, ctx) {
    const publicTargets = JSON.parse(env.PUBLIC_TARGETS_JSON);
    const deniedTargets = JSON.parse(env.DENIED_TARGETS_JSON);
    const allowed = await Promise.all(publicTargets.map(async target => {
      try {
        const response = await fetch(target, { signal: AbortSignal.timeout(3000) });
        if (!response.ok) throw new Error(`public fixture status ${response.status}`);
        return await response.text();
      } catch (error) {
        return { target, error: String(error && error.message || error) };
      }
    }));
    const deniedResults = await Promise.all(deniedTargets.map(async target => {
      try {
        await fetch(target, { signal: AbortSignal.timeout(1000) });
        return false;
      } catch {
        return true;
      }
    }));
    const denied = deniedResults.filter(Boolean).length;
    const ctxExports = await loopbackProbe(ctx.exports.SocketService, "ctx.exports");
    const rawTcp = env.RAW_TCP_CONFIG_JSON
      ? await rawTcpMatrix(JSON.parse(env.RAW_TCP_CONFIG_JSON))
      : null;
    return Response.json({ allowed, denied, ctxExports, rawTcp });
  },

  async queue(batch, env) {
    await eventSourceRawTcp(env, "queue");
    batch.ackAll();
  },

  async scheduled(controller, env) {
    await eventSourceRawTcp(env, "scheduled");
    controller.noRetry();
  },
};
