import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const { BrowserTransport } = await importRuntime("browser/host.ts", {
  "cloudflare:workers": moduleUrl("export class WorkerEntrypoint {}"),
  "../loader/shared.js": moduleUrl(
    `export const BINDING_TOKEN_HEADER = "x-open-compute-binding-token"; export const currentStartupGeneration = () => "current-generation"; export const bindingError = code => new Error(code);`,
  ),
});

test("BrowserTransport overwrites authority and strips caller credentials for HTTP and WebSocket", async () => {
  const transport = new BrowserTransport();
  transport.ctx = {
    props: {
      instanceId: "instance",
      workerId: "worker",
      versionId: "version",
      deploymentId: "deployment",
      bindingName: "BROWSER",
      descriptorSha256: "a".repeat(64),
      capabilityVersion: 1,
    },
  };
  transport.env = {
    BINDING_BACKEND_TOKEN: "private-token",
    BINDING_BACKEND: {
      fetch: async (request) => {
        assert.equal(
          request.url,
          "http://binding-backend/internal/browser/v1/devtools/browser/session?keep_alive=60000",
        );
        assert.equal(
          request.headers.get("x-open-compute-binding-token"),
          "private-token",
        );
        assert.equal(
          request.headers.get("x-open-compute-startup-generation"),
          "current-generation",
        );
        for (const [name, value] of [
          ["instance-id", "instance"],
          ["worker-id", "worker"],
          ["version-id", "version"],
          ["deployment-id", "deployment"],
          ["binding-name", "BROWSER"],
          ["descriptor-sha256", "a".repeat(64)],
          ["capability-version", "1"],
        ])
          assert.equal(request.headers.get(`x-open-compute-${name}`), value);
        assert.equal(request.headers.get("authorization"), null);
        assert.equal(request.headers.get("cookie"), null);
        assert.equal(request.headers.get("upgrade"), "websocket");
        return new Response("ok");
      },
    },
  };
  const response = await transport.fetch(
    new Request(
      "https://untrusted.host/v1/devtools/browser/session?keep_alive=60000",
      {
        headers: {
          authorization: "tenant-secret",
          cookie: "tenant-cookie",
          Upgrade: "websocket",
          "x-open-compute-binding-token": "forged",
          "x-open-compute-instance-id": "other",
          "x-open-compute-descriptor-sha256": "forged",
        },
      },
    ),
  );
  assert.equal(await response.text(), "ok");
  assert.throws(
    () => transport.fetch(new Request("https://browser/internal/ready")),
    /BROWSER_UNSUPPORTED/,
  );
  assert.throws(
    () =>
      transport.fetch(
        new Request("https://browser/v1/sessions", {
          headers: { "cf-brapi-guardrails": "unsupported-policy" },
        }),
      ),
    /BROWSER_UNSUPPORTED/,
  );
  transport.ctx.props.capabilityVersion = 2;
  assert.throws(
    () => transport.fetch(new Request("https://browser/v1/sessions")),
    /BROWSER_UNAVAILABLE/,
  );
});
