import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";
const { default: gateway } = await importRuntime("gateway/ingress.ts");

test("private ingress forwards authenticated WebSocket upgrades only on dispatch", async () => {
  const forwarded = [];
  const env = {
    INTERNAL_TOKEN: "test-generation",
    LOADER_HOST: { async fetch(request) { forwarded.push(request); return new Response("accepted"); } },
  };
  const make = (path, token, upgrade = "websocket") => new Request(`http://private${path}`, {
    headers: { "x-open-compute-internal-token": token, upgrade },
  });
  for (const request of [
    make("/internal/dispatch", "old-generation"),
    make("/internal/dispatch?extra=1", "test-generation"),
    make("/internal/validate", "test-generation"),
    make("/internal/dispatch", "test-generation", "other"),
  ]) assert.equal((await gateway.fetch(request, env)).status, 404);
  assert.equal(forwarded.length, 0);
  assert.equal((await gateway.fetch(make("/internal/dispatch", "test-generation"), env)).status, 200);
  assert.equal(forwarded[0].method, "GET");
  assert.equal(forwarded[0].headers.get("upgrade"), "websocket");
});
