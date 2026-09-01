import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const {
  inboundSocketTargetAddress,
  socketAddressFromWire,
  socketAuthorityWire,
  validateSocketAuthorityWire,
} = await importRuntime("sockets/tunnel.ts", {});

test("typed socket authority preserves IPv6 record and string forms across private hops", () => {
  const record = socketAuthorityWire({ hostname: "2606:4700:4700::1111", port: 443 });
  assert.deepEqual(record, {
    kind: "record",
    hostname: "2606:4700:4700::1111",
    port: 443,
  });
  assert.deepEqual(
    socketAddressFromWire(record, "2606:4700:4700::1111:443"),
    { hostname: "2606:4700:4700::1111", port: 443 },
  );

  const string = socketAuthorityWire("[2606:4700:4700::1111]:443");
  assert.deepEqual(string, {
    kind: "string",
    address: "[2606:4700:4700::1111]:443",
  });
  assert.equal(
    socketAddressFromWire(string, "[2606:4700:4700::1111]:443"),
    "[2606:4700:4700::1111]:443",
  );
});

test("private socket authority rejects confused, malformed, and extended wire values", () => {
  assert.throws(
    () => socketAddressFromWire(
      { kind: "record", hostname: "2606:4700:4700::1111", port: 443 },
      "example.com:443",
    ),
    /SOCKET_TUNNEL_INVALID/,
  );
  for (const value of [
    null,
    { kind: "string", address: "x" },
    { kind: "record", hostname: "example.com", port: -1 },
    { kind: "record", hostname: "example.com", port: 443, extra: true },
    { kind: "other", address: "example.com:443" },
  ]) {
    assert.throws(() => validateSocketAuthorityWire(value), /SOCKET_TUNNEL_INVALID/);
  }
});

test("inbound CONNECT authority recovers workerd's unbracketed IPv6 form", async () => {
  assert.deepEqual(
    await inboundSocketTargetAddress({ opened: Promise.resolve({ localAddress: "2606:4700:4700::1111:443" }) }),
    { hostname: "2606:4700:4700::1111", port: 443 },
  );
  assert.equal(
    await inboundSocketTargetAddress({ opened: Promise.resolve({ localAddress: "example.com:443" }) }),
    "example.com:443",
  );
});
