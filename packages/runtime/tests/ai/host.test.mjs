import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const cloudflare = moduleUrl(`
  export class WorkerEntrypoint {
    constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  }
`);
const privateTransport = moduleUrl(`
  export const isRecord = value => value !== null && typeof value === "object" && !Array.isArray(value);
  export async function bindingJson(response) { return response.json(); }
  export async function expectBindingStatus(response, status) {
    if (response.status !== status) throw new Error("AI_PROTOCOL_ERROR");
  }
`);
const shared = moduleUrl(`
  export const BINDING_TOKEN_HEADER = "x-open-compute-binding-token";
  export const currentStartupGeneration = () => "generation";
  export const systemRequestId = () => "018f0000-0000-7000-8000-000000000001";
  export const bindingError = code => Object.assign(new Error(code), { stableCode: code });
`);
const { AiTransport } = await import(moduleUrl(await compileRuntime("ai/host.ts", {
  "cloudflare:workers": cloudflare,
  "../bindings/private-transport.js": privateTransport,
  "../loader/shared.js": shared,
})));

const props = {
  accountId: "account", workerId: "worker", versionId: "version",
  descriptorSha256: "ab".repeat(32),
};

test("AI transport sends only the private authorized Markdown Conversion wire contract", async () => {
  const calls = [];
  const env = {
    BINDING_BACKEND_TOKEN: "private-token",
    BINDING_BACKEND: { async fetch(url, init) {
      calls.push({ url, init, body: init.body && JSON.parse(init.body) });
      return Response.json({ schemaVersion: 1, result: url.endsWith("/supported") ? [] : [{ ok: true }] });
    } },
  };
  const transport = new AiTransport({ props }, env);
  assert.deepEqual(await transport.transform([
    { name: "manual.pdf", mimeType: "application/pdf", dataBase64: "cGRm" },
  ], { output: { format: "markdown" } }), [{ ok: true }]);
  assert.deepEqual(await transport.supported(), []);
  assert.deepEqual(calls.map(call => [call.init.method, new URL(call.url).pathname]), [
    ["POST", "/internal/ai/to-markdown/v1/transform"],
    ["GET", "/internal/ai/to-markdown/v1/supported"],
  ]);
  assert.deepEqual(calls[0].body, {
    schemaVersion: 1,
    files: [{ name: "manual.pdf", mimeType: "application/pdf", dataBase64: "cGRm" }],
    options: { output: { format: "markdown" } },
  });
  assert.equal(calls[0].init.headers["x-open-compute-binding-token"], "private-token");
  assert.equal(calls[0].init.headers["x-open-compute-descriptor-sha256"], props.descriptorSha256);
  assert.equal(calls[0].init.headers["x-open-compute-startup-generation"], "generation");
});

test("AI transport maps backend and malformed envelope failures to stable codes", async () => {
  const failed = new AiTransport({ props }, {
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: { fetch: async () => new Response(null, {
      status: 422, headers: { "x-open-compute-error-code": "DOCUMENT_ENCRYPTED" },
    }) },
  });
  await assert.rejects(failed.supported(), error => error.stableCode === "DOCUMENT_ENCRYPTED");
  const malformed = new AiTransport({ props }, {
    BINDING_BACKEND_TOKEN: "token",
    BINDING_BACKEND: { fetch: async () => Response.json({ result: [] }) },
  });
  await assert.rejects(malformed.supported(), error => error.stableCode === "AI_PROTOCOL_ERROR");
});
