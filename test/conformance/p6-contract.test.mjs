import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { validateCommitted } from "./p6-contract.mjs";
import { assertSanitizedTrace, sanitizeTrace } from "./p6-trace.mjs";

const capability = JSON.parse(readFileSync(new URL("../../openapi/p6-capability.json", import.meta.url)));
const extension = JSON.parse(readFileSync(new URL("../../openapi/open-compute-extension.json", import.meta.url)));
const catalog = JSON.parse(readFileSync(new URL("./catalog.json", import.meta.url)));

test("P6 pinned OpenAPI subset and capability projection are internally reproducible", () => {
  assert.doesNotThrow(() => validateCommitted({
    wranglerRoot: new URL("../../packages/toolchain/node_modules/wrangler/", import.meta.url).pathname,
    sdkRoot: new URL("../../packages/cloudflare-extension/node_modules/cloudflare/", import.meta.url).pathname,
  }));
  assert.equal(catalog.managementApi.routeCount, capability.managementApi.routes.length);
  assert.deepEqual(catalog.managementApi.deviations, capability.managementApi.deviations);
  assert.equal(catalog.wrangler.fieldCount, capability.wrangler.fields.length);
  assert.equal(catalog.wrangler.bindingCount, capability.wrangler.bindings.length);
  assert.equal(catalog.wrangler.commandCount, capability.wrangler.commands.length);
});

test("vendor extension operations have stable typed envelopes and exact request media", () => {
  const operations = Object.entries(extension.paths).flatMap(([path, methods]) =>
    Object.entries(methods).map(([method, operation]) => ({ path, method, operation })),
  );
  assert.equal(operations.length, 18);
  assert.equal(operations.filter(({ method }) => method === "post").length, 8);
  assert.equal(new Set(operations.map(({ operation }) => operation.operationId)).size, 18);
  assert.ok(operations.every(({ operation }) => operation["x-open-compute-capability-status"] === "supported"));
  const restores = operations.filter(({ operation }) => operation.operationId.endsWith("-restore"));
  assert.equal(restores.length, 2);
  assert.ok(restores.every(({ operation }) =>
    operation["x-open-compute-request-body"] === "json"
      && operation.requestBody.required === true
      && operation.requestBody.content["application/json"].schema.$ref
        === "#/components/schemas/RestoreRequest"));
  assert.ok(operations.filter(({ operation }) => !operation.operationId.endsWith("-restore"))
    .every(({ operation }) => operation["x-open-compute-request-body"] === "none"
      && operation.requestBody === undefined));
  for (const { operation } of operations) {
    assert.equal(operation.responses["4XX"].content["application/json"].schema.$ref,
      "#/components/schemas/ErrorEnvelope");
    assert.equal(operation.responses["5XX"].content["application/json"].schema.$ref,
      "#/components/schemas/ErrorEnvelope");
    const success = Object.entries(operation.responses).filter(([status]) => status.startsWith("2"));
    assert.ok(success.length >= 1);
    for (const [, response] of success) {
      const name = response.content["application/json"].schema.$ref.split("/").at(-1);
      const schema = extension.components.schemas[name];
      assert.equal(schema.properties.success.const, true);
      assert.notDeepEqual(schema.properties.result, {});
    }
  }
  assert.equal(extension.components.schemas.ErrorEnvelope.properties.success.const, false);
  assert.equal(extension.components.schemas.ErrorEnvelope.properties.result.type, "null");
  assert.equal(extension.components.schemas.ErrorEnvelope.properties.errors.minItems, 1);
  assert.deepEqual(extension.components.schemas.RestoredResource.properties.kind.enum,
    ["kv_namespace", "d1_database"]);
});

test("settings surfaces, asset upload variants, and old routes are classified exactly", () => {
  const routes = new Map(capability.managementApi.routes.map(item => [item.id, item]));
  assert.equal(capability.managementApi.routes.filter(item => item.status === "supported").length, 149);
  assert.equal(capability.managementApi.routes.filter(item => item.status === "supported_with_deviation").length, 2);
  assert.equal(capability.managementApi.routes.filter(item => item.status === "planned").length, 3);
  assert.equal(capability.managementApi.routes.filter(item => item.status === "unsupported").length, 1);
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/settings")?.requestMediaType, "multipart");
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/script-settings")?.requestMediaType, "json");
  assert.equal(routes.get("PATCH /accounts/{account_id}/workers/scripts/{script_name}/secrets-bulk")?.operationId,
    "worker-patch-script-secrets-bulk");
  assert.deepEqual(
    [routes.get("GET /accounts/{account_id}/workers/subdomain")?.status,
      routes.get("GET /accounts/{account_id}/workers/subdomain")?.operationId],
    ["supported_with_deviation", "worker-subdomain-get-subdomain"],
  );
  assert.match(
    routes.get("GET /accounts/{account_id}/workers/subdomain")?.constraint ?? "",
    /non-DNS label/,
  );
  assert.deepEqual(
    routes.get("GET /accounts/{account_id}/workers/subdomain")?.deviations,
    ["OC-ACCOUNT-SUBDOMAIN-001"],
  );
  assert.deepEqual(
    [routes.get("GET /accounts/{account_id}/ai-search/tokens")?.status,
      routes.get("GET /accounts/{account_id}/ai-search/tokens")?.deviations],
    ["supported_with_deviation", ["OC-AI-SEARCH-TOKEN-001"]],
  );
  assert.deepEqual(capability.managementApi.deviations,
    ["OC-ACCOUNT-SUBDOMAIN-001", "OC-AI-SEARCH-TOKEN-001"]);
  assert.deepEqual(
    [routes.get("POST /accounts/{account_id}/workers/assets/upload/{manifest_hash}")?.status,
      routes.get("POST /accounts/{account_id}/workers/assets/upload/{manifest_hash}")?.source],
    ["supported", "wrangler-dist/cli.js:157069"],
  );
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
