import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { validateCommitted } from "./p6-contract.mjs";
import { assertSanitizedTrace, sanitizeTrace } from "./p6-trace.mjs";

const capability = JSON.parse(readFileSync(new URL("../../openapi/p6-capability.json", import.meta.url)));
const catalog = JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url)));

test("P6 pinned OpenAPI subset and capability projection are internally reproducible", () => {
  assert.doesNotThrow(() => validateCommitted());
  assert.equal(catalog.managementApi.routeCount, capability.managementApi.routes.length);
  assert.equal(catalog.wrangler.fieldCount, capability.wrangler.fields.length);
  assert.equal(catalog.wrangler.bindingCount, capability.wrangler.bindings.length);
  assert.equal(catalog.wrangler.commandCount, capability.wrangler.commands.length);
});

test("settings surfaces, asset upload variants, and old routes are classified exactly", () => {
  const routes = new Map(capability.managementApi.routes.map(item => [item.id, item]));
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/settings")?.requestMediaType, "multipart");
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/script-settings")?.requestMediaType, "json");
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/secrets-bulk")?.operationId,
    "worker-patch-script-secrets-bulk");
  assert.equal(routes.get("POST /accounts/{account_id}/workers/assets/upload/{manifest_hash}")?.source,
    "wrangler-dist/cli.js:157069");
  assert.equal(routes.get("GET /accounts/{account_id}/r2/buckets/{bucket_name}/objects")?.status, "unsupported");
  assert.ok(capability.managementApi.legacyRoutes.every(item => item.status === "unsupported"));
});

test("P7, P8, and P9 fields remain explicit handoffs without support claims", () => {
  const fields = new Map(capability.wrangler.fields.map(item => [item.id, item]));
  const bindings = new Map(capability.wrangler.bindings.map(item => [item.id, item]));
  const commands = new Map(capability.wrangler.commands.map(item => [item.id, item]));
  assert.deepEqual([fields.get("observability")?.status, fields.get("observability")?.stage], ["planned", "P7"]);
  assert.deepEqual([fields.get("limits.cpu_ms")?.status, fields.get("limits.cpu_ms")?.stage], ["unsupported", "P8"]);
  assert.deepEqual([fields.get("limits.subrequests")?.status, fields.get("limits.subrequests")?.stage], ["unsupported", "P8"]);
  assert.deepEqual([fields.get("worker_loaders[].binding")?.status, fields.get("worker_loaders[].binding")?.stage], ["unsupported", "P9"]);
  assert.deepEqual([bindings.get("worker_loader")?.status, bindings.get("worker_loader")?.stage], ["unsupported", "P9"]);
  assert.deepEqual([commands.get("tail")?.status, commands.get("tail")?.stage], ["planned", "P7"]);
  assert.equal(fields.get("usage_model")?.source, "pinned-schema-absence");
});

test("trace sanitizer removes credentials, multipart boundaries, and secret JSON values", () => {
  const sanitized = sanitizeTrace({
    method: "patch",
    path: "/accounts/0123456789abcdef0123456789abcdef/workers/scripts/example/script-settings",
    headers: {
      Authorization: "Bearer api-token",
      Cookie: "session=super-secret",
      "Content-Type": "multipart/form-data; boundary=signed-upload-token",
    },
    body: JSON.stringify({ name: "ok", secret: "super-secret", nested: { jwt: "signed-upload-token" } }),
  });
  assert.equal(sanitized.path, "/accounts/{account_id}/workers/scripts/example/script-settings");
  assert.equal(sanitized.headers.authorization, "<redacted>");
  assert.equal(sanitized.headers["content-type"], "multipart/form-data; boundary=<boundary>");
  assertSanitizedTrace(sanitized);
});
