import assert from "node:assert/strict";
import test from "node:test";
import { Ingress } from "./runtime.mjs";

test("private ingress forwards WebSocket upgrades only on dispatch", async () => {
  const forwarded = [];
  const env = {
    LOADER_HOST: {
      async fetch(request) {
        forwarded.push(request);
        return new Response("accepted");
      },
    },
  };
  const gateway = new Ingress({}, env);
  const make = (path, upgrade = "websocket") =>
    new Request(`http://private${path}`, {
      headers: { upgrade },
    });
  for (const request of [
    make("/internal/dispatch?extra=1"),
    make("/internal/validate"),
    make("/internal/dispatch", "other"),
  ])
    assert.equal((await gateway.fetch(request)).status, 404);
  assert.equal(forwarded.length, 0);
  assert.equal((await gateway.fetch(make("/internal/dispatch"))).status, 200);
  assert.equal(forwarded[0].method, "GET");
  assert.equal(forwarded[0].headers.get("upgrade"), "websocket");
});
