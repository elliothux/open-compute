import assert from "node:assert/strict";
import { connect as tlsConnect } from "node:tls";
import { connect as socketConnect } from "cloudflare:sockets";

const HOST = "p0-2-public.test";
const encoder = new TextEncoder();

function payload(size = 192 * 1024) {
  return Uint8Array.from({ length: size }, (_value, index) => index % 251);
}

async function readWebEcho(readable, expected) {
  const reader = readable.getReader();
  let offset = 0;
  let chunks = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      chunks++;
      for (const value of result.value) {
        assert.equal(value, expected[offset]);
        offset++;
      }
    }
  } finally {
    reader.releaseLock();
  }
  assert.equal(offset, expected.byteLength);
  assert.ok(chunks > 1);
}

async function webSocketEcho(socket) {
  const expected = payload();
  await socket.opened;
  const receiving = readWebEcho(socket.readable, expected);
  const writer = socket.writable.getWriter();
  try {
    await writer.write(encoder.encode(`ECHO ${expected.byteLength}\n`));
    for (let offset = 0; offset < expected.byteLength; offset += 32 * 1024) {
      await writer.write(expected.subarray(offset, offset + 32 * 1024));
    }
    await writer.close();
  } finally {
    writer.releaseLock();
  }
  await receiving;
  await socket.close();
  await socket.closed;
}

function nodeTlsEcho(port) {
  const expected = payload();
  return new Promise((resolve, reject) => {
    const socket = tlsConnect({
      host: HOST,
      port,
      servername: HOST,
      rejectUnauthorized: true,
      allowHalfOpen: true,
    });
    let offset = 0;
    let chunks = 0;
    let settled = false;
    const fail = error => {
      if (settled) return;
      settled = true;
      socket.destroy();
      reject(error);
    };
    socket.once("error", fail);
    socket.on("data", chunk => {
      chunks++;
      for (const value of chunk) {
        if (offset >= expected.byteLength || value !== expected[offset]) {
          fail(new Error("node:tls echo mismatch"));
          return;
        }
        offset++;
      }
    });
    socket.once("end", () => {
      if (settled) return;
      try {
        assert.equal(offset, expected.byteLength);
        assert.ok(chunks > 1);
        assert.equal(socket.authorized, true);
        socket.destroy();
        assert.equal(socket.destroyed, true);
        settled = true;
        resolve();
      } catch (error) {
        fail(error);
      }
    });
    socket.once("secureConnect", () => {
      socket.write(`ECHO ${expected.byteLength}\n`);
      for (let offset = 0; offset < expected.byteLength; offset += 32 * 1024) {
        socket.write(expected.subarray(offset, offset + 32 * 1024));
      }
      socket.end();
    });
  });
}

function nodeTlsTimeout(port) {
  return new Promise((resolve, reject) => {
    const socket = tlsConnect({
      host: HOST,
      port,
      servername: HOST,
      rejectUnauthorized: true,
    });
    socket.once("secureConnect", () => {
      assert.equal(socket.authorized, true);
      socket.write("STALL\n");
      socket.setTimeout(100);
    });
    socket.once("timeout", () => {
      socket.destroy();
      assert.equal(socket.destroyed, true);
      resolve();
    });
    socket.once("error", reject);
  });
}

export const cloudflareTlsOn = {
  async test(_control, env) {
    const port = Number(env.TLS_PORT);
    const socket = socketConnect(`${HOST}:${port}`, {
      allowHalfOpen: true,
      secureTransport: "on",
    });
    assert.equal(socket.secureTransport, "on");
    assert.equal(socket.upgraded, false);
    await webSocketEcho(socket);
  },
};

export const cloudflareStartTls = {
  async test(_control, env) {
    const port = Number(env.TLS_PORT);
    const initial = socketConnect(`${env.PUBLIC_IPV4}:${port}`, {
      allowHalfOpen: true,
      secureTransport: "starttls",
    });
    assert.equal(initial.secureTransport, "starttls");
    await initial.opened;
    const upgraded = initial.startTls({ expectedServerHostname: HOST });
    assert.throws(() => initial.startTls(), /already been called|closed|transferred/i);
    await webSocketEcho(upgraded);
    assert.equal(initial.upgraded, true);
    assert.equal(upgraded.secureTransport, "on");
  },
};

export const nodeTlsLifecycle = {
  async test(_control, env) {
    const port = Number(env.TLS_PORT);
    await nodeTlsEcho(port);
    await nodeTlsTimeout(port);
  },
};
