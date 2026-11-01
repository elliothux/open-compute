import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  buildCapability,
  officialCfSchema,
  validateCommitted,
} from "./p6-contract.mjs";
import { assertSanitizedTrace, sanitizeTrace } from "./p6-trace.mjs";

const capability = JSON.parse(
  readFileSync(new URL("../../openapi/p6-capability.json", import.meta.url)),
);
const extension = JSON.parse(
  readFileSync(
    new URL("../../openapi/open-compute-extension.json", import.meta.url),
  ),
);
const catalog = JSON.parse(
  readFileSync(new URL("./catalog.json", import.meta.url)),
);

test("P6 pinned OpenAPI subset and capability projection are internally reproducible", () => {
  assert.doesNotThrow(() =>
    validateCommitted({
      cfRoot: new URL("../../node_modules/cf/", import.meta.url).pathname,
      sdkRoot: new URL(
        "../../packages/sdk/node_modules/cloudflare/",
        import.meta.url,
      ).pathname,
    }),
  );
  assert.equal(
    catalog.managementApi.routeCount,
    capability.managementApi.routes.length,
  );
  assert.deepEqual(
    catalog.managementApi.deviations,
    capability.managementApi.deviations,
  );
  assert.equal(catalog.cf.fieldCount, capability.cf.fields.length);
  assert.equal(catalog.cf.bindingCount, capability.cf.bindings.length);
  assert.equal(catalog.cf.commandCount, capability.cf.commands.length);
});

test("vendor extension operations have stable typed envelopes and exact request media", () => {
  const operations = Object.entries(extension.paths).flatMap(
    ([path, methods]) =>
      Object.entries(methods).map(([method, operation]) => ({
        path,
        method,
        key: `${method.toUpperCase()} ${path}`,
        operation,
      })),
  );
  assert.equal(operations.length, 46);
  assert.equal(operations.filter(({ method }) => method === "post").length, 14);
  assert.equal(
    new Set(operations.map(({ operation }) => operation.operationId)).size,
    46,
  );
  assert.ok(
    operations.every(
      ({ operation }) =>
        operation["x-open-compute-capability-status"] === "supported",
    ),
  );
  const restores = operations.filter(({ operation }) =>
    operation.operationId.endsWith("-restore"),
  );
  assert.equal(restores.length, 2);
  assert.ok(
    restores.every(
      ({ operation }) =>
        operation["x-open-compute-request-body"] === "json" &&
        operation.requestBody.required === true &&
        operation.requestBody.content["application/json"].schema.$ref ===
          "#/components/schemas/RestoreRequest",
    ),
  );
  const migrations = operations.filter(({ key }) =>
    key.endsWith("/open-compute/d1/databases/{database_id}/migrations"),
  );
  assert.equal(migrations.length, 2);
  assert.equal(migrations[0].operation["x-open-compute-request-body"], "none");
  assert.equal(migrations[0].operation.requestBody, undefined);
  assert.equal(migrations[1].operation["x-open-compute-request-body"], "json");
  assert.equal(
    migrations[1].operation.requestBody.content["application/json"].schema.$ref,
    "#/components/schemas/D1MigrationRequest",
  );
  const publicOrigin = operations.filter(({ key }) =>
    key.endsWith("/open-compute/workers/{script_name}/public-origin"),
  );
  assert.deepEqual(
    publicOrigin.map(({ method }) => method),
    ["get", "put", "delete"],
  );
  assert.equal(
    publicOrigin[1].operation.requestBody.content["application/json"].schema
      .$ref,
    "#/components/schemas/PublicOriginRequest",
  );
  assert.equal(publicOrigin[0].operation.requestBody, undefined);
  assert.equal(publicOrigin[2].operation.requestBody, undefined);
  assert.ok(
    operations
      .filter(
        ({ operation }) => operation["x-open-compute-request-body"] === "none",
      )
      .every(
        ({ operation }) =>
          operation["x-open-compute-request-body"] === "none" &&
          operation.requestBody === undefined,
      ),
  );
  for (const { operation } of operations) {
    assert.equal(
      operation.responses["4XX"].content["application/json"].schema.$ref,
      "#/components/schemas/ErrorEnvelope",
    );
    assert.equal(
      operation.responses["5XX"].content["application/json"].schema.$ref,
      "#/components/schemas/ErrorEnvelope",
    );
    const success = Object.entries(operation.responses).filter(([status]) =>
      status.startsWith("2"),
    );
    assert.ok(success.length >= 1);
    for (const [, response] of success) {
      const name = response.content["application/json"].schema.$ref
        .split("/")
        .at(-1);
      const schema = extension.components.schemas[name];
      assert.equal(schema.properties.success.const, true);
      assert.notDeepEqual(schema.properties.result, {});
    }
  }
  assert.equal(
    extension.components.schemas.ErrorEnvelope.properties.success.const,
    false,
  );
  assert.equal(
    extension.components.schemas.ErrorEnvelope.properties.result.type,
    "null",
  );
  assert.equal(
    extension.components.schemas.ErrorEnvelope.properties.errors.minItems,
    1,
  );
  assert.deepEqual(
    extension.components.schemas.RestoredResource.properties.kind.enum,
    ["kv_namespace", "d1_database"],
  );
});

test("settings surfaces, bulk asset upload, and old routes are classified exactly", () => {
  const routes = new Map(
    capability.managementApi.routes.map((item) => [item.id, item]),
  );
  assert.equal(
    capability.managementApi.routes.filter(
      (item) => item.status === "supported",
    ).length,
    200,
  );
  assert.equal(
    capability.managementApi.routes.filter(
      (item) => item.status === "supported_with_deviation",
    ).length,
    26,
  );
  assert.equal(
    capability.managementApi.routes.filter((item) => item.status === "planned")
      .length,
    0,
  );
  assert.equal(
    capability.managementApi.routes.filter(
      (item) => item.status === "unsupported",
    ).length,
    0,
  );
  assert.equal(
    routes.get(
      "PATCH /accounts/{account_id}/workers/scripts/{script_name}/settings",
    )?.requestMediaType,
    "multipart",
  );
  assert.equal(
    routes.get(
      "PATCH /accounts/{account_id}/workers/scripts/{script_name}/script-settings",
    )?.requestMediaType,
    "json",
  );
  assert.equal(
    routes.get(
      "PATCH /accounts/{account_id}/workers/scripts/{script_name}/secrets-bulk",
    )?.operationId,
    "worker-patch-script-secrets-bulk",
  );
  assert.deepEqual(
    [
      routes.get("GET /accounts/{account_id}/workers/subdomain")?.status,
      routes.get("GET /accounts/{account_id}/workers/subdomain")?.operationId,
    ],
    ["supported_with_deviation", "worker-subdomain-get-subdomain"],
  );
  assert.match(
    routes.get("GET /accounts/{account_id}/workers/subdomain")?.constraint ??
      "",
    /non-DNS label/,
  );
  assert.deepEqual(
    routes.get("GET /accounts/{account_id}/workers/subdomain")?.deviations,
    ["OC-ACCOUNT-SUBDOMAIN-001"],
  );
  assert.deepEqual(
    [
      routes.get("GET /accounts/{account_id}/ai-search/tokens")?.status,
      routes.get("GET /accounts/{account_id}/ai-search/tokens")?.deviations,
    ],
    ["supported_with_deviation", ["OC-AI-SEARCH-TOKEN-001"]],
  );
  for (const id of [
    "POST /accounts/{account_id}/workers/scripts/{script_name}/tails",
    "POST /accounts/{account_id}/workers/observability/telemetry/keys",
    "POST /accounts/{account_id}/workers/observability/telemetry/values",
    "POST /accounts/{account_id}/workers/observability/telemetry/query",
    "POST /accounts/{account_id}/workers/observability/telemetry/live-tail",
  ]) {
    assert.deepEqual(
      [routes.get(id)?.status, routes.get(id)?.deviations],
      ["supported_with_deviation", ["OC-OBSERVABILITY-001"]],
    );
  }
  assert.deepEqual(
    [
      routes.get(
        "GET /accounts/{account_id}/d1/database/{database_id}/time_travel/bookmark",
      )?.status,
      routes.get(
        "POST /accounts/{account_id}/d1/database/{database_id}/time_travel/restore",
      )?.status,
    ],
    ["supported_with_deviation", "supported_with_deviation"],
  );
  assert.deepEqual(
    routes.get(
      "GET /accounts/{account_id}/d1/database/{database_id}/time_travel/bookmark",
    )?.deviations,
    ["OC-D1-001"],
  );
  assert.match(
    routes.get(
      "POST /accounts/{account_id}/d1/database/{database_id}/time_travel/restore",
    )?.constraint ?? "",
    /not automatically retained restore points/,
  );
  assert.deepEqual(capability.managementApi.deviations, [
    "OC-ACCOUNT-SUBDOMAIN-001",
    "OC-DEPLOY-001",
    "OC-D1-001",
    "OC-AI-SEARCH-TOKEN-001",
    "OC-OBSERVABILITY-001",
    "OC-CONTAINERS-MGMT-001",
  ]);
  assert.deepEqual(
    [
      routes.get("POST /accounts/{account_id}/workers/assets/upload")?.status,
      routes.get("POST /accounts/{account_id}/workers/assets/upload")?.source,
    ],
    ["supported", "cloudflare-openapi"],
  );
  assert.equal(
    routes.get("GET /accounts/{account_id}/r2/buckets/{bucket_name}/objects")
      ?.status,
    "supported",
  );
  assert.ok(
    capability.managementApi.legacyRoutes.every(
      (item) => item.status === "unsupported",
    ),
  );
});

test("implemented P7 fields and later handoffs remain explicit", () => {
  const fields = new Map(capability.cf.fields.map((item) => [item.id, item]));
  const bindings = new Map(
    capability.cf.bindings.map((item) => [item.id, item]),
  );
  const commands = new Map(
    capability.cf.commands.map((item) => [item.id, item]),
  );
  assert.deepEqual(
    [fields.get("observability")?.status, fields.get("observability")?.source],
    ["supported", "cf-config-schema"],
  );
  assert.deepEqual(
    [fields.get("limits.cpuMs")?.status, fields.get("limits.cpuMs")?.stage],
    ["supported", undefined],
  );
  assert.deepEqual(
    [
      fields.get("limits.subrequests")?.status,
      fields.get("limits.subrequests")?.stage,
    ],
    ["supported", undefined],
  );
  assert.equal(fields.get("env")?.status, "supported");
  assert.equal(bindings.get("worker_loader")?.status, "supported");
  assert.equal(commands.get("deploy")?.status, "supported");
  assert.equal(commands.has("tail"), false);
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
    body: JSON.stringify({
      name: "ok",
      secret: "super-secret",
      nested: { jwt: "signed-upload-token" },
    }),
  });
  assert.equal(
    sanitized.path,
    "/accounts/{account_id}/workers/scripts/example/script-settings",
  );
  assert.equal(sanitized.headers.authorization, "<redacted>");
  assert.equal(
    sanitized.headers["content-type"],
    "multipart/form-data; boundary=<boundary>",
  );
  assertSanitizedTrace(sanitized);
});

test("Container management inventory records the implemented cf subset and SDK boundary", () => {
  const routes = capability.managementApi.routes.filter((route) =>
    route.path.includes("/containers/"),
  );
  assert.equal(routes.length, 13);
  assert.equal(new Set(routes.map((route) => route.id)).size, routes.length);
  for (const route of routes) {
    assert.equal(route.status, "supported_with_deviation");
    assert.equal(route.source, "cf-1.0.0-beta.12-containers");
    assert.deepEqual(route.deviations, ["OC-CONTAINERS-MGMT-001"]);
    assert.equal(
      route.requestMediaType,
      ["POST", "PATCH"].includes(route.method) ? "json" : "none",
    );
  }
  const ids = new Set(routes.map((route) => route.id));
  for (const operation of [
    "GET /accounts/{account_id}/containers/applications/{application_id}/instances-v2",
    "GET /accounts/{account_id}/containers/applications/{application_id}/instances/{instance_id}",
    "POST /accounts/{account_id}/containers/registries/{domain}/credentials",
    "POST /accounts/{account_id}/containers/image-preparations",
  ])
    assert.ok(ids.has(operation));
});

test("management authority rejects a duplicate observed Container route", () => {
  const read = (path) =>
    JSON.parse(readFileSync(new URL(path, import.meta.url)));
  const manifest = read("../../openapi/cloudflare-subset-manifest.json");
  const container = manifest.clientObservedOperations.find((entry) =>
    entry.operation.includes("/containers/"),
  );
  assert.ok(container);
  assert.throws(
    () =>
      buildCapability(
        read("../../openapi/cloudflare-v4-subset.json"),
        {
          ...manifest,
          clientObservedOperations: [
            ...manifest.clientObservedOperations,
            container,
          ],
        },
        read("../../openapi/p6-capability-source.json"),
        "1.0.0-beta.12",
        "unused",
        {},
      ),
    /duplicate management capability route/,
  );
});

test("route annotations cover product groups, validate docs/cf and derive SDK use", () => {
  const read = (path) =>
    JSON.parse(readFileSync(new URL(path, import.meta.url)));
  const source = read("../../openapi/p6-capability-source.json");
  const schema = officialCfSchema(
    new URL("../../node_modules/cf/", import.meta.url).pathname,
  );
  const build = (input = source) =>
    buildCapability(
      read("../../openapi/cloudflare-v4-subset.json"),
      read("../../openapi/cloudflare-subset-manifest.json"),
      input,
      capability.cf.version,
      capability.cf.configSchemaSha256,
      schema,
    ).managementApi;
  const { routes, routeGroups } = build();
  const route = (id) => routes.find((item) => item.id === id);
  assert.equal(
    route("POST /accounts/{account_id}/workers/scripts/{script_name}/versions")
      .group,
    "workers-versions",
  );
  const r2 = route("PUT /accounts/{account_id}/r2/buckets/{bucket_name}");
  assert.equal(routeGroups[r2.group].cf, "emits");
  assert.equal(r2.sdk, "not-found");
  const service = route(
    "GET /accounts/{account_id}/workers/services/{script_name}",
  );
  assert.equal(service.cf, "emits");
  assert.match(service.docs, /#service-environments$/);
  assert.equal(routeGroups["open-compute"].docs, null);
  assert.deepEqual(
    routeGroups["workers-scripts"].uploadFields.map((field) => field.id),
    [
      "body_part",
      "bindings.wasm_module",
      "bindings.text_blob",
      "bindings.data_blob",
      "bindings.d1.id",
      "bindings.queue.delivery_delay",
    ],
  );
  const surface = read("../../packages/sdk/surface.json");
  for (const item of surface.operations)
    assert.equal(route(item.operation).sdk, "official");
  for (const item of [
    ...surface.openComputeOperations,
    ...surface.observedStandardOperations,
  ])
    assert.equal(route(`${item.method} ${item.path}`).sdk, "open-compute");
  for (const item of surface.excludedOperations)
    assert.equal(route(item.operation).sdk, "not-found");

  for (const [mutate, error] of [
    [(api) => delete api.routeGroups.kv, /unannotated management route/],
    [
      (api) => (api.routeGroups.kv.docs = "https://example.com/kv/"),
      /invalid route annotation docs/,
    ],
    [
      (api) => (api.routeGroups.kv.cf = "unverified"),
      /invalid route annotation cf/,
    ],
    [
      (api) => (api.routeGroups["workers-scripts"].uploadFields[0].cf = "bad"),
      /invalid route annotation cf/,
    ],
    [
      (api) => (api.routeOverrides[service.id].docs = null),
      /invalid route annotation docs/,
    ],
  ]) {
    const invalid = structuredClone(source);
    mutate(invalid.managementApi);
    assert.throws(() => build(invalid), error);
  }
});
