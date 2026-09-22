import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const LOCK_PATH = join(ROOT, "openapi/upstream/cloudflare-openapi.lock.json");
const MANIFEST_PATH = join(ROOT, "openapi/cloudflare-subset-manifest.json");
const SUBSET_PATH = join(ROOT, "openapi/cloudflare-v4-subset.json");
const CAPABILITY_PATH = join(ROOT, "openapi/p6-capability.json");
const CAPABILITY_SOURCE_PATH = join(ROOT, "openapi/p6-capability-source.json");
const EXTENSION_PATH = join(ROOT, "openapi/open-compute-extension.json");

function json(path) {
  return JSON.parse(readFileSync(path, "utf8"));
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

// Committed JSON authority is the repository-prettier form; both the writer
// and the byte-reproducibility checks canonicalize through it so generator,
// check, and pre-commit hooks agree on the exact bytes.
export function prettierJson(text) {
  const formatted = spawnSync(
    join(ROOT, "node_modules/.bin/prettier"),
    ["--stdin-filepath", "authority.json"],
    { input: text, cwd: ROOT, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 },
  );
  if (formatted.error) throw formatted.error;
  if (formatted.status !== 0)
    throw new Error(
      `prettier failed for the authority JSON: ${formatted.stderr}`,
    );
  return formatted.stdout;
}

function operationKey(value) {
  const separator = value.indexOf(" ");
  if (separator < 1) throw new Error(`invalid operation key: ${value}`);
  return [value.slice(0, separator).toLowerCase(), value.slice(separator + 1)];
}

function atPointer(document, pointer) {
  let value = document;
  for (const part of pointer.slice(2).split("/")) {
    value = value[part.replaceAll("~1", "/").replaceAll("~0", "~")];
    if (value === undefined)
      throw new Error(`missing OpenAPI reference: ${pointer}`);
  }
  return value;
}

function addPointer(output, pointer, value) {
  let target = output;
  const parts = pointer
    .slice(2)
    .split("/")
    .map((part) => part.replaceAll("~1", "/").replaceAll("~0", "~"));
  for (const part of parts.slice(0, -1)) target = target[part] ??= {};
  target[parts.at(-1)] = value;
}

function collectRefs(value, refs) {
  if (Array.isArray(value)) {
    for (const item of value) collectRefs(item, refs);
    return;
  }
  if (value === null || typeof value !== "object") return;
  if (typeof value.$ref === "string" && value.$ref.startsWith("#/components/"))
    refs.add(value.$ref);
  for (const item of Object.values(value)) collectRefs(item, refs);
}

export function buildSubset(openapi, manifest, revision, sourceSha256) {
  const paths = {};
  const refs = new Set();
  const operationInventory = [];
  const seen = new Set();
  const deferred = new Map(
    manifest.deferredOperations.map((item) => [item.operation, item]),
  );
  const unsupported = new Map(
    manifest.unsupportedOperations.map((item) => [item.operation, item]),
  );
  const deviated = new Map(
    (manifest.supportedWithDeviationOperations ?? []).map((item) => [
      item.operation,
      item,
    ]),
  );
  const selectedOperations = [
    ...manifest.operations,
    ...deferred.keys(),
    ...unsupported.keys(),
  ];
  for (const key of deviated.keys()) {
    if (!manifest.operations.includes(key)) {
      throw new Error(
        `supported-with-deviation operation is not selected: ${key}`,
      );
    }
  }
  for (const key of selectedOperations) {
    if (seen.has(key)) throw new Error(`duplicate selected operation: ${key}`);
    seen.add(key);
    const [method, path] = operationKey(key);
    const sourcePath = openapi.paths?.[path];
    const operation = sourcePath?.[method];
    if (operation === undefined)
      throw new Error(`selected operation is absent upstream: ${key}`);
    const selectedPath = (paths[path] ??= {});
    if (sourcePath.parameters !== undefined)
      selectedPath.parameters = sourcePath.parameters;
    selectedPath[method] = operation;
    collectRefs(selectedPath, refs);
    operationInventory.push({
      id: key,
      method: method.toUpperCase(),
      path,
      operationId: operation.operationId,
      status: unsupported.has(key)
        ? "unsupported"
        : deferred.has(key)
          ? "planned"
          : deviated.has(key)
            ? "supported_with_deviation"
            : "supported",
      source: "cloudflare-openapi",
      ...(deferred.has(key) ? { stage: deferred.get(key).stage } : {}),
      ...(deviated.has(key) ? { constraint: deviated.get(key).reason } : {}),
      ...(deviated.has(key)
        ? { deviations: deviated.get(key).deviations }
        : {}),
      operationSha256: sha256(`${JSON.stringify(operation)}\n`),
    });
  }
  const components = {};
  const complete = new Set();
  while (refs.size > complete.size) {
    for (const pointer of [...refs]) {
      if (complete.has(pointer)) continue;
      const value = atPointer(openapi, pointer);
      addPointer({ components }, pointer, value);
      collectRefs(value, refs);
      complete.add(pointer);
    }
  }
  return {
    openapi: openapi.openapi,
    info: {
      title: "open-compute Cloudflare v4 selected contract",
      version: openapi.info?.version,
      description:
        "Mechanically selected from the pinned official Cloudflare OpenAPI snapshot; do not hand-edit.",
    },
    "x-open-compute-upstream": { revision, sha256: sourceSha256 },
    "x-open-compute-operation-inventory": operationInventory,
    paths,
    components,
  };
}

function requestMediaType(operation) {
  const content = Object.keys(operation.requestBody?.content ?? {});
  if (content.length === 0) return "none";
  if (content.some((value) => value.startsWith("multipart/")))
    return "multipart";
  if (
    content.some(
      (value) => value === "application/json" || value.endsWith("+json"),
    )
  )
    return "json";
  return "raw";
}

export function buildCapability(
  subset,
  manifest,
  source,
  configSchemaSha256,
  configSchema,
) {
  const operations = new Map(
    subset["x-open-compute-operation-inventory"].map((item) => [item.id, item]),
  );
  const routes = manifest.operations.map((id) => {
    const item = operations.get(id);
    const operation = subset.paths[item.path][item.method.toLowerCase()];
    return { ...item, requestMediaType: requestMediaType(operation) };
  });
  for (const item of manifest.deferredOperations) {
    const operation = operations.get(item.operation);
    const body = subset.paths[operation.path][operation.method.toLowerCase()];
    routes.push({
      ...operation,
      status: item.status,
      source: "cloudflare-openapi",
      stage: item.stage,
      requestMediaType: requestMediaType(body),
    });
  }
  for (const item of manifest.unsupportedOperations) {
    const [method, path] = operationKey(item.operation);
    routes.push({
      id: item.operation,
      method: method.toUpperCase(),
      path,
      status: item.status,
      source: "cloudflare-openapi",
      constraint: item.reason,
      requestMediaType: "none",
    });
  }
  for (const item of manifest.wranglerObservedOperations) {
    const [method, path] = operationKey(item.operation);
    routes.push({
      id: item.operation,
      method: method.toUpperCase(),
      path,
      status: item.status,
      source: item.source,
      constraint: item.note,
      requestMediaType: "multipart",
    });
  }
  for (const item of manifest.clientObservedOperations ?? []) {
    const [method, path] = operationKey(item.operation);
    routes.push({
      id: item.operation,
      method: method.toUpperCase(),
      path,
      status: item.status,
      source: item.source,
      constraint: item.note,
      deviations: item.deviations ?? [],
      requestMediaType: item.requestMediaType ?? "json",
    });
  }
  for (const id of source.managementApi.vendorRoutes) {
    const [method, path] = operationKey(id);
    routes.push({
      id,
      method: method.toUpperCase(),
      path,
      operationId: EXTENSION_OPERATIONS[id][0],
      status: "supported",
      source: "open-compute-extension",
      requestMediaType: REQUEST_SCHEMAS.has(id) ? "json" : "none",
    });
  }
  const declaredDeviations = new Set(source.managementApi.deviations);
  const referencedDeviations = new Set();
  for (const route of routes) {
    const deviations = route.deviations ?? [];
    if (
      (route.status === "supported_with_deviation") !==
      deviations.length > 0
    ) {
      throw new Error(`route deviation status/link mismatch: ${route.id}`);
    }
    if (new Set(deviations).size !== deviations.length) {
      throw new Error(`duplicate route deviation: ${route.id}`);
    }
    for (const deviation of deviations) {
      if (!declaredDeviations.has(deviation)) {
        throw new Error(
          `route references undeclared deviation: ${route.id} -> ${deviation}`,
        );
      }
      referencedDeviations.add(deviation);
    }
  }
  for (const deviation of declaredDeviations) {
    if (!referencedDeviations.has(deviation)) {
      throw new Error(`unreferenced management deviation: ${deviation}`);
    }
  }
  const topFields = Object.keys(
    configSchema.definitions?.RawConfig?.properties ?? {},
  );
  if (topFields.length === 0)
    throw new Error("Wrangler RawConfig field inventory is empty");
  const statusByField = new Map();
  for (const id of source.wrangler.supportedFields)
    statusByField.set(id, {
      status: "supported",
      source: "wrangler-config-schema",
    });
  for (const [id, stage] of Object.entries(source.wrangler.deferredFields))
    statusByField.set(id, {
      status: "planned",
      source: "wrangler-config-schema",
      stage,
    });
  for (const [id, stage] of Object.entries(
    source.wrangler.unsupportedDeferredFields,
  ))
    statusByField.set(id, {
      status: "unsupported",
      source: "wrangler-config-schema",
      stage,
    });
  for (const id of topFields)
    if (!statusByField.has(id))
      statusByField.set(id, {
        status: "unsupported",
        source: "wrangler-config-schema",
      });
  for (const id of statusByField.keys())
    if (!topFields.includes(id))
      throw new Error(`unknown Wrangler top-level field: ${id}`);
  const fields = topFields.map((id) => ({ id, ...statusByField.get(id) }));
  fields.push(
    {
      id: "observability.logs",
      status: "supported",
      source: "wrangler-config-schema",
    },
    {
      id: "observability.traces",
      status: "unsupported",
      source: "wrangler-config-schema",
      stage: "P7",
    },
    {
      id: "limits.cpu_ms",
      status: "unsupported",
      source: "wrangler-config-schema",
      stage: "P8",
    },
    {
      id: "limits.subrequests",
      status: "unsupported",
      source: "wrangler-config-schema",
      stage: "P8",
    },
    {
      id: "usage_model",
      status: "unsupported",
      source: "pinned-schema-absence",
      stage: "P8",
      constraint:
        "wrangler@4.127.1 config-schema.json has no usage_model property",
    },
    {
      id: "worker_loaders[].binding",
      status: "supported",
      source: "wrangler-config-schema",
    },
  );
  const bindings = [
    ...source.wrangler.supportedBindings.map((id) => ({
      id,
      status: "supported",
      source: "wrangler-multipart",
    })),
    ...source.wrangler.unsupportedBindings.map((id) => ({
      id,
      status: "unsupported",
      source: "wrangler-multipart",
    })),
  ];
  const commands = [
    ...source.wrangler.supportedCommands.map((id) => ({
      id,
      status: "supported",
      source: "wrangler-cli",
    })),
    ...Object.entries(source.wrangler.deferredCommands).map(([id, stage]) => ({
      id,
      status: "planned",
      source: "wrangler-cli",
      stage,
    })),
    ...source.wrangler.unsupportedCommands.map((id) => ({
      id,
      status: "unsupported",
      source: "wrangler-cli",
    })),
  ];
  return {
    schemaVersion: 1,
    managementApi: {
      root: source.managementApi.root,
      routes,
      deviations: source.managementApi.deviations,
      legacyRoutes: source.managementApi.legacyRoutes.map((id) => ({
        id,
        status: "unsupported",
        source: "day1-negative-route-inventory",
      })),
    },
    workersObservability: source.workersObservability,
    workerLoader: source.workerLoader,
    wrangler: {
      version: "4.127.1",
      configSchemaSha256,
      fields,
      bindings,
      commands,
    },
  };
}

// Each entry carries the vendor `operationId`, the success envelope schema,
// the success status codes, and the generated SDK method tree under
// `client.openCompute` (surfaced as `x-open-compute-sdk-method`).
const EXTENSION_OPERATIONS = {
  "GET /open-compute/capabilities": [
    "open-compute-get-open-compute-capabilities",
    "CapabilitiesResponse",
    ["200"],
    "capabilities.get",
  ],
  "GET /open-compute/system/status": [
    "open-compute-get-open-compute-system-status",
    "SystemStatusResponse",
    ["200"],
    "system.status",
  ],
  "GET /open-compute/scheduler": [
    "open-compute-get-open-compute-scheduler",
    "SchedulerStatusResponse",
    ["200"],
    "scheduler.get",
  ],
  "POST /open-compute/scheduler/pause": [
    "open-compute-post-open-compute-scheduler-pause",
    "SchedulerStatusResponse",
    ["200"],
    "scheduler.pause",
  ],
  "POST /open-compute/scheduler/resume": [
    "open-compute-post-open-compute-scheduler-resume",
    "SchedulerStatusResponse",
    ["200"],
    "scheduler.resume",
  ],
  "POST /open-compute/scheduler/repair": [
    "open-compute-post-open-compute-scheduler-repair",
    "SchedulerStatusResponse",
    ["200"],
    "scheduler.repair",
  ],
  "GET /open-compute/cache": [
    "open-compute-get-open-compute-cache",
    "CacheStatusResponse",
    ["200"],
    "cache.get",
  ],
  "POST /open-compute/cache/garbage-collection": [
    "open-compute-post-open-compute-cache-garbage-collection",
    "CacheStatusResponse",
    ["200"],
    "cache.collectGarbage",
  ],
  "GET /open-compute/images/capacity": [
    "open-compute-get-open-compute-images-capacity",
    "ImageCapacityResponse",
    ["200"],
    "images.capacity",
  ],
  "GET /open-compute/upgrade/check": [
    "open-compute-get-open-compute-upgrade-check",
    "UpgradeCheckResponse",
    ["200"],
    "upgrade.check",
  ],
  "GET /accounts/{account_id}/open-compute/workers/{script_name}/endpoints": [
    "open-compute-get-accounts-account-id-open-compute-workers-script-name-endpoints",
    "WorkerEndpointsResponse",
    ["200"],
    "workers.endpoints",
  ],
  "GET /accounts/{account_id}/open-compute/workers/{script_name}/public-origin":
    [
      "open-compute-get-accounts-account-id-open-compute-workers-script-name-public-origin",
      "PublicOriginBindingResponse",
      ["200"],
      "workers.publicOrigin.get",
    ],
  "PUT /accounts/{account_id}/open-compute/workers/{script_name}/public-origin":
    [
      "open-compute-put-accounts-account-id-open-compute-workers-script-name-public-origin",
      "WorkerEndpointResponse",
      ["200"],
      "workers.publicOrigin.set",
    ],
  "DELETE /accounts/{account_id}/open-compute/workers/{script_name}/public-origin":
    [
      "open-compute-delete-accounts-account-id-open-compute-workers-script-name-public-origin",
      "NullResponse",
      ["200"],
      "workers.publicOrigin.delete",
    ],
  "GET /accounts/{account_id}/open-compute/durable-objects": [
    "open-compute-get-accounts-account-id-open-compute-durable-objects",
    "DurableObjectNamespacesResponse",
    ["200"],
    "durableObjects.list",
  ],
  "GET /accounts/{account_id}/open-compute/durable-objects/{namespace_id}/objects":
    [
      "open-compute-get-accounts-account-id-open-compute-durable-objects-namespace-id-objects",
      "DurableObjectRecordsResponse",
      ["200"],
      "durableObjects.objects",
    ],
  "POST /accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups":
    [
      "open-compute-post-accounts-account-id-open-compute-kv-namespaces-namespace-id-backups",
      "BackupResponse",
      ["200", "201"],
      "backups.kv.create",
    ],
  "GET /accounts/{account_id}/open-compute/kv/namespaces/{namespace_id}/backups":
    [
      "open-compute-get-accounts-account-id-open-compute-kv-namespaces-namespace-id-backups",
      "BackupsResponse",
      ["200"],
      "backups.kv.list",
    ],
  "POST /accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore": [
    "open-compute-post-accounts-account-id-open-compute-kv-backups-backup-id-restore",
    "RestoredResourceResponse",
    ["200", "201"],
    "backups.kv.restore",
  ],
  "POST /accounts/{account_id}/open-compute/d1/databases/{database_id}/backups":
    [
      "open-compute-post-accounts-account-id-open-compute-d1-databases-database-id-backups",
      "BackupResponse",
      ["200", "201"],
      "backups.d1.create",
    ],
  "GET /accounts/{account_id}/open-compute/d1/databases/{database_id}/backups":
    [
      "open-compute-get-accounts-account-id-open-compute-d1-databases-database-id-backups",
      "BackupsResponse",
      ["200"],
      "backups.d1.list",
    ],
  "POST /accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore": [
    "open-compute-post-accounts-account-id-open-compute-d1-backups-backup-id-restore",
    "RestoredResourceResponse",
    ["200", "201"],
    "backups.d1.restore",
  ],
  "GET /accounts/{account_id}/open-compute/d1/databases/{database_id}/migrations":
    [
      "open-compute-get-accounts-account-id-open-compute-d1-databases-database-id-migrations",
      "D1MigrationsResponse",
      ["200"],
      "d1.migrations.list",
    ],
  "PUT /accounts/{account_id}/open-compute/d1/databases/{database_id}/migrations":
    [
      "open-compute-put-accounts-account-id-open-compute-d1-databases-database-id-migrations",
      "D1MigrationsResponse",
      ["200"],
      "d1.migrations.apply",
    ],
};

const REQUEST_SCHEMAS = new Map([
  [
    "PUT /accounts/{account_id}/open-compute/workers/{script_name}/public-origin",
    "PublicOriginRequest",
  ],
  [
    "POST /accounts/{account_id}/open-compute/kv/backups/{backup_id}/restore",
    "RestoreRequest",
  ],
  [
    "POST /accounts/{account_id}/open-compute/d1/backups/{backup_id}/restore",
    "RestoreRequest",
  ],
  [
    "PUT /accounts/{account_id}/open-compute/d1/databases/{database_id}/migrations",
    "D1MigrationRequest",
  ],
]);

function successEnvelope(result) {
  return {
    type: "object",
    additionalProperties: false,
    required: ["success", "result", "errors", "messages"],
    properties: {
      success: { type: "boolean", const: true },
      result,
      errors: {
        type: "array",
        maxItems: 0,
        items: { $ref: "#/components/schemas/Error" },
      },
      messages: {
        type: "array",
        items: { $ref: "#/components/schemas/Message" },
      },
    },
  };
}

function objectSchema(required, properties) {
  return { type: "object", additionalProperties: false, required, properties };
}

function extensionSchemas() {
  const string = { type: "string" };
  const nonNegativeInteger = { type: "integer", minimum: 0 };
  const schemas = {
    PathSegment: { type: "string", minLength: 1, pattern: "^(?!\\.{1,2}$).+$" },
    Error: objectSchema(["code", "message"], {
      code: { type: "integer", minimum: 9_100_000, maximum: 9_199_999 },
      message: string,
      source: objectSchema([], { pointer: string }),
    }),
    Message: objectSchema(["code", "message"], {
      code: { type: "integer" },
      message: string,
    }),
    Capabilities: objectSchema(
      [
        "release",
        "wrangler_version",
        "compatibility_date",
        "compatibility_flags",
        "endpoints",
        "deviations",
      ],
      {
        release: { type: "string", minLength: 1 },
        wrangler_version: { type: "string", const: "4.127.1" },
        compatibility_date: objectSchema(["minimum", "maximum"], {
          minimum: { type: "string", format: "date" },
          maximum: { type: "string", format: "date" },
        }),
        compatibility_flags: {
          type: "array",
          uniqueItems: true,
          items: string,
        },
        endpoints: {
          type: "object",
          additionalProperties: {
            type: "string",
            enum: ["supported", "supported_with_deviation", "unsupported"],
          },
        },
        deviations: { type: "array", uniqueItems: true, items: string },
      },
    ),
    SystemStatus: objectSchema(
      ["state", "version", "components", "operator_proxy"],
      {
        state: string,
        version: string,
        components: {
          type: "array",
          items: objectSchema(["name", "state"], {
            name: string,
            state: string,
            message: string,
          }),
        },
        operator_proxy: objectSchema(["mode"], {
          mode: { type: "string", enum: ["direct", "proxy", "invalid"] },
          source_variable: string,
          origin: string,
        }),
        observability: objectSchema(
          [
            "state",
            "retention_ms",
            "max_database_bytes",
            "query_max_timeframe_ms",
            "tail_sessions",
            "closed_client_drops",
            "overload_drops",
          ],
          {
            state: { type: "string", enum: ["healthy", "degraded"] },
            retention_ms: nonNegativeInteger,
            max_database_bytes: nonNegativeInteger,
            query_max_timeframe_ms: nonNegativeInteger,
            oldest_event_ms: nonNegativeInteger,
            tail_sessions: nonNegativeInteger,
            closed_client_drops: nonNegativeInteger,
            overload_drops: nonNegativeInteger,
          },
        ),
        deployment_runtime: objectSchema(
          ["dispatchable", "quarantined", "active_runtime_dispatchable"],
          {
            dispatchable: nonNegativeInteger,
            quarantined: nonNegativeInteger,
            active_runtime_dispatchable: { type: "boolean" },
            last_quarantine_reason: string,
          },
        ),
      },
    ),
    SchedulerStatus: objectSchema(["state", "pending", "running"], {
      state: string,
      pending: nonNegativeInteger,
      running: nonNegativeInteger,
    }),
    CacheStatus: objectSchema(["entries", "bytes"], {
      entries: nonNegativeInteger,
      bytes: nonNegativeInteger,
    }),
    ImageCapacity: objectSchema(["queued", "running", "capacity"], {
      queued: nonNegativeInteger,
      running: nonNegativeInteger,
      capacity: nonNegativeInteger,
    }),
    WorkerEndpoint: objectSchema(["id", "kind", "url", "scope", "created_on"], {
      id: string,
      kind: { type: "string", enum: ["local_origin", "public_origin"] },
      url: { type: "string", format: "uri" },
      scope: { type: "string", enum: ["local_machine", "public_network"] },
      created_on: { type: "string", format: "date-time" },
    }),
    PublicOriginRequest: objectSchema(["name"], {
      name: {
        type: "string",
        maxLength: 63,
        pattern: "^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$",
      },
    }),
    DurableObjectNamespace: objectSchema(["id", "script_name", "class_name"], {
      id: string,
      script_name: string,
      class_name: string,
    }),
    DurableObjectRecord: objectSchema(["id", "namespace_id", "created_on"], {
      id: string,
      namespace_id: string,
      created_on: { type: "string", format: "date-time" },
    }),
    Backup: objectSchema(["id", "created_on", "state"], {
      id: string,
      created_on: { type: "string", format: "date-time" },
      state: string,
      size: nonNegativeInteger,
    }),
    RestoreRequest: objectSchema(["name"], {
      name: {
        type: "string",
        minLength: 1,
        maxLength: 128,
        pattern: "^[^\\u0000-\\u001F\\u007F]+$",
      },
    }),
    D1Migration: objectSchema(["id", "name", "sha256", "applied_at_ms"], {
      id: { type: "integer", minimum: 1 },
      name: { type: "string", minLength: 1, maxLength: 255 },
      sha256: { type: "string", pattern: "^[0-9a-f]{64}$" },
      applied_at_ms: { type: "integer", minimum: 0 },
    }),
    D1MigrationInput: objectSchema(["id", "name", "sha256", "sql"], {
      id: { type: "integer", minimum: 1 },
      name: { type: "string", minLength: 1, maxLength: 255 },
      sha256: { type: "string", pattern: "^[0-9a-f]{64}$" },
      sql: { type: "string", minLength: 1 },
    }),
    D1MigrationRequest: {
      type: "array",
      minItems: 1,
      items: { $ref: "#/components/schemas/D1MigrationInput" },
    },
    RestoredResource: objectSchema(["id", "name", "kind", "created_on"], {
      id: string,
      name: { type: "string", minLength: 1, maxLength: 128 },
      kind: { type: "string", enum: ["kv_namespace", "d1_database"] },
      created_on: { type: "string", format: "date-time" },
    }),
    UpgradeCheck: objectSchema(
      [
        "schema_version",
        "current_version",
        "update_available",
        "upgrade_allowed",
      ],
      {
        schema_version: nonNegativeInteger,
        current_version: string,
        available_version: { type: ["string", "null"], minLength: 1 },
        update_available: { type: "boolean" },
        upgrade_allowed: { type: "boolean" },
        blocked_reason: { type: ["string", "null"], minLength: 1 },
      },
    ),
  };
  schemas.ErrorEnvelope = objectSchema(
    ["success", "result", "errors", "messages"],
    {
      success: { type: "boolean", const: false },
      result: { type: "null" },
      errors: {
        type: "array",
        minItems: 1,
        items: { $ref: "#/components/schemas/Error" },
      },
      messages: {
        type: "array",
        items: { $ref: "#/components/schemas/Message" },
      },
    },
  );
  for (const [name, result] of Object.entries({
    CapabilitiesResponse: { $ref: "#/components/schemas/Capabilities" },
    SystemStatusResponse: { $ref: "#/components/schemas/SystemStatus" },
    SchedulerStatusResponse: { $ref: "#/components/schemas/SchedulerStatus" },
    CacheStatusResponse: { $ref: "#/components/schemas/CacheStatus" },
    ImageCapacityResponse: { $ref: "#/components/schemas/ImageCapacity" },
    WorkerEndpointsResponse: {
      type: "array",
      items: { $ref: "#/components/schemas/WorkerEndpoint" },
    },
    WorkerEndpointResponse: { $ref: "#/components/schemas/WorkerEndpoint" },
    PublicOriginBindingResponse: {
      type: ["object", "null"],
      additionalProperties: false,
      required: ["name", "url"],
      properties: {
        name: { type: "string" },
        url: { type: "string", format: "uri" },
      },
    },
    NullResponse: { type: "null" },
    DurableObjectNamespacesResponse: {
      type: "array",
      items: { $ref: "#/components/schemas/DurableObjectNamespace" },
    },
    DurableObjectRecordsResponse: {
      type: "array",
      items: { $ref: "#/components/schemas/DurableObjectRecord" },
    },
    BackupResponse: { $ref: "#/components/schemas/Backup" },
    BackupsResponse: {
      type: "array",
      items: { $ref: "#/components/schemas/Backup" },
    },
    RestoredResourceResponse: { $ref: "#/components/schemas/RestoredResource" },
    D1MigrationsResponse: {
      type: "array",
      items: { $ref: "#/components/schemas/D1Migration" },
    },
    UpgradeCheckResponse: { $ref: "#/components/schemas/UpgradeCheck" },
  }))
    schemas[name] = successEnvelope(result);
  return schemas;
}

export function buildExtension(source) {
  const expected = new Set(source.managementApi.vendorRoutes);
  if (
    expected.size !== Object.keys(EXTENSION_OPERATIONS).length ||
    Object.keys(EXTENSION_OPERATIONS).some((id) => !expected.has(id))
  ) {
    throw new Error(
      "vendor extension operation contract differs from the route authority",
    );
  }
  const paths = {};
  const sdkMethods = new Set();
  for (const id of source.managementApi.vendorRoutes) {
    const [method, path] = operationKey(id);
    const [operationId, responseSchema, successStatuses, sdkMethod] =
      EXTENSION_OPERATIONS[id];
    if (sdkMethods.has(sdkMethod)) {
      throw new Error(`duplicate vendor SDK method tree: ${sdkMethod}`);
    }
    sdkMethods.add(sdkMethod);
    if (!/^[a-z][a-zA-Z0-9]*(\.[a-z][a-zA-Z0-9]*)*$/.test(sdkMethod)) {
      throw new Error(`invalid vendor SDK method tree: ${sdkMethod}`);
    }
    const parameters = [...path.matchAll(/\{([^}]+)\}/g)].map((match) => ({
      name: match[1],
      in: "path",
      required: true,
      schema: { $ref: "#/components/schemas/PathSegment" },
    }));
    paths[path] ??= {};
    const requestSchema = REQUEST_SCHEMAS.get(id);
    paths[path][method] = {
      operationId,
      "x-open-compute-capability-status": "supported",
      "x-open-compute-sdk-method": sdkMethod,
      parameters,
      "x-open-compute-request-body":
        requestSchema === undefined ? "none" : "json",
      ...(requestSchema !== undefined
        ? {
            requestBody: {
              required: true,
              content: {
                "application/json": {
                  schema: { $ref: `#/components/schemas/${requestSchema}` },
                },
              },
            },
          }
        : {}),
      responses: Object.fromEntries([
        ...successStatuses.map((status) => [
          status,
          {
            description: "Successful vendor extension response",
            content: {
              "application/json": {
                schema: { $ref: `#/components/schemas/${responseSchema}` },
              },
            },
          },
        ]),
        ...["4XX", "5XX"].map((status) => [
          status,
          {
            description: "Cloudflare-style or vendor-reserved error response",
            content: {
              "application/json": {
                schema: { $ref: "#/components/schemas/ErrorEnvelope" },
              },
            },
          },
        ]),
      ]),
    };
  }
  return {
    openapi: "3.1.0",
    info: {
      title: "open-compute Cloudflare v4 extension",
      version: "0.1.0",
      description:
        "Source schema for vendor-only routes. Planned operations are not an implementation claim.",
    },
    servers: [{ url: "/client/v4" }],
    paths,
    components: { schemas: extensionSchemas() },
  };
}

function walk(root) {
  const files = [];
  for (const name of readdirSync(root)) {
    const path = join(root, name);
    if (statSync(path).isDirectory()) files.push(...walk(path));
    else files.push(path);
  }
  return files;
}

function sdkRoutes(sdkRoot) {
  const routes = new Set();
  for (const path of walk(join(sdkRoot, "resources"))) {
    if (!path.endsWith(".mjs")) continue;
    const source = readFileSync(path, "utf8");
    for (const match of source.matchAll(
      /`(\/accounts\/\$\{account_id\}[^`]*)`/g,
    )) {
      routes.add(
        match[1]
          .replaceAll("${account_id}", "{account_id}")
          .replaceAll(/\$\{(?:scriptName|script_name)\}/g, "{script_name}")
          .replaceAll("${versionID}", "{version_id}")
          .replaceAll("${deploymentID}", "{deployment_id}"),
      );
    }
  }
  return routes;
}

export function validateCommitted({ openapiPath, wranglerRoot, sdkRoot } = {}) {
  const lock = json(LOCK_PATH);
  const manifest = json(MANIFEST_PATH);
  if (
    sha256(readFileSync(MANIFEST_PATH)) !== lock.subsetManifestSha256 ||
    sha256(readFileSync(EXTENSION_PATH)) !== lock.extensionSha256 ||
    sha256(readFileSync(CAPABILITY_PATH)) !== lock.capabilitySha256 ||
    sha256(
      readFileSync(join(ROOT, "openapi/capability-manifest.schema.json")),
    ) !== lock.capabilitySchemaSha256
  ) {
    throw new Error("P6 contract authority digest drift");
  }
  const subsetBytes = readFileSync(SUBSET_PATH);
  if (lock.subsetSha256 !== sha256(subsetBytes))
    throw new Error("generated OpenAPI subset digest drift");
  const subset = JSON.parse(subsetBytes);
  const inventory = subset["x-open-compute-operation-inventory"];
  const selectedIds = [
    ...manifest.operations,
    ...manifest.deferredOperations.map((item) => item.operation),
    ...manifest.unsupportedOperations.map((item) => item.operation),
  ];
  if (inventory.length !== selectedIds.length)
    throw new Error("subset operation inventory is incomplete");
  const ids = inventory.map((item) => item.id);
  if (
    new Set(ids).size !== ids.length ||
    ids.join("\0") !== selectedIds.join("\0")
  ) {
    throw new Error("subset operations differ from the selection manifest");
  }
  const capability = json(CAPABILITY_PATH);
  const extension = json(EXTENSION_PATH);
  const capabilitySource = json(CAPABILITY_SOURCE_PATH);
  const expectedExtension = prettierJson(
    `${JSON.stringify(buildExtension(capabilitySource), null, 2)}\n`,
  );
  if (expectedExtension !== readFileSync(EXTENSION_PATH, "utf8")) {
    throw new Error("vendor extension schema is not reproducible");
  }
  const expectedVendorPaths = new Set(
    capabilitySource.managementApi.vendorRoutes.map(
      (id) => operationKey(id)[1],
    ),
  );
  if (
    Object.keys(extension.paths).length !== expectedVendorPaths.size ||
    Object.keys(extension.paths).some((path) => !expectedVendorPaths.has(path))
  ) {
    throw new Error("vendor extension schema route inventory drift");
  }
  const extensionOperations = Object.entries(extension.paths).flatMap(
    ([path, methods]) =>
      Object.entries(methods).map(([method, operation]) => ({
        key: `${method.toUpperCase()} ${path}`,
        operation,
      })),
  );
  const operationIds = extensionOperations.map(
    ({ operation }) => operation.operationId,
  );
  if (
    extensionOperations.length !==
      capabilitySource.managementApi.vendorRoutes.length ||
    new Set(operationIds).size !== operationIds.length
  ) {
    throw new Error(
      "vendor extension operation IDs are incomplete or duplicated",
    );
  }
  for (const { key, operation } of extensionOperations) {
    const requestSchema = REQUEST_SCHEMAS.get(key);
    if (
      operation["x-open-compute-capability-status"] !== "supported" ||
      operation["x-open-compute-request-body"] !==
        (requestSchema === undefined ? "none" : "json") ||
      (requestSchema !== undefined
        ? operation.requestBody?.content?.["application/json"]?.schema?.$ref !==
          `#/components/schemas/${requestSchema}`
        : operation.requestBody !== undefined)
    ) {
      throw new Error(
        `vendor extension operation contract drift: ${operation.operationId}`,
      );
    }
    for (const [status, response] of Object.entries(operation.responses)) {
      const reference = response.content?.["application/json"]?.schema?.$ref;
      if (status === "4XX" || status === "5XX") {
        if (reference !== "#/components/schemas/ErrorEnvelope") {
          throw new Error(
            `vendor extension error envelope drift: ${operation.operationId}`,
          );
        }
      } else if (
        !status.startsWith("2") ||
        reference === undefined ||
        reference.endsWith("/ErrorEnvelope")
      ) {
        throw new Error(
          `vendor extension success envelope drift: ${operation.operationId}`,
        );
      }
    }
  }
  const routeIds = capability.managementApi.routes.map((item) => item.id);
  for (const id of [
    ...manifest.operations,
    ...manifest.deferredOperations.map((item) => item.operation),
    ...manifest.unsupportedOperations.map((item) => item.operation),
    ...manifest.wranglerObservedOperations.map((item) => item.operation),
  ]) {
    if (!routeIds.includes(id))
      throw new Error(`capability route inventory is missing ${id}`);
  }
  for (const item of capability.managementApi.routes) {
    if (!lockStatus(item.status))
      throw new Error(`invalid route status: ${item.id}`);
  }
  for (const collection of [
    capability.wrangler.fields,
    capability.wrangler.bindings,
    capability.wrangler.commands,
  ]) {
    if (new Set(collection.map((item) => item.id)).size !== collection.length)
      throw new Error("duplicate capability item");
    for (const item of collection)
      if (!lockStatus(item.status))
        throw new Error(`invalid capability status: ${item.id}`);
  }
  const observability = capability.workersObservability;
  for (const collection of [
    observability.settingsFields,
    observability.features,
    observability.queryViews,
    observability.queryOperators,
  ]) {
    if (new Set(collection.map((item) => item.id)).size !== collection.length) {
      throw new Error("duplicate Workers observability capability item");
    }
    for (const item of collection) {
      if (!lockStatus(item.status))
        throw new Error(`invalid Workers observability status: ${item.id}`);
      const deviations = item.deviations ?? [];
      if (
        (item.status === "supported_with_deviation") !==
          deviations.length > 0 ||
        deviations.some((value) => !observability.deviations.includes(value))
      ) {
        throw new Error(
          `invalid Workers observability deviation link: ${item.id}`,
        );
      }
    }
  }
  if (
    observability.scriptTailProtocol !== "trace-v1" ||
    new Set(observability.limits).size !== observability.limits.length ||
    observability.limits.length === 0
  ) {
    throw new Error(
      "Workers observability protocol or limit authority is invalid",
    );
  }
  if (
    capability.wrangler.version !== lock.wrangler.version ||
    capability.wrangler.configSchemaSha256 !== lock.wrangler.configSchemaSha256
  ) {
    throw new Error("Wrangler capability identity differs from the fixed lock");
  }
  if (openapiPath !== undefined) {
    const bytes = readFileSync(openapiPath);
    if (sha256(bytes) !== lock.sha256)
      throw new Error("official OpenAPI snapshot digest mismatch");
    const rebuilt = prettierJson(
      `${JSON.stringify(buildSubset(JSON.parse(bytes), manifest, lock.revision, lock.sha256), null, 2)}\n`,
    );
    if (!Buffer.from(rebuilt).equals(subsetBytes))
      throw new Error("committed OpenAPI subset is not reproducible");
  }
  if (wranglerRoot !== undefined) {
    if (
      sha256(readFileSync(join(wranglerRoot, "config-schema.json"))) !==
        lock.wrangler.configSchemaSha256 ||
      sha256(readFileSync(join(wranglerRoot, "wrangler-dist/cli.js"))) !==
        lock.wrangler.cliSha256
    ) {
      throw new Error("installed Wrangler does not match the fixed contract");
    }
    const source = readFileSync(
      join(wranglerRoot, "wrangler-dist/cli.js"),
      "utf8",
    );
    for (const required of [
      'bindings_inherit: "strict"',
      "/script-settings`,",
      "/workers/assets/upload/${manifestEntry[1].hash}",
    ])
      if (!source.includes(required))
        throw new Error(`Wrangler trace authority is missing ${required}`);
    const expected = prettierJson(
      `${JSON.stringify(
        buildCapability(
          subset,
          manifest,
          json(CAPABILITY_SOURCE_PATH),
          lock.wrangler.configSchemaSha256,
          json(join(wranglerRoot, "config-schema.json")),
        ),
        null,
        2,
      )}\n`,
    );
    if (expected !== readFileSync(CAPABILITY_PATH, "utf8"))
      throw new Error("P6 capability projection is not reproducible");
  }
  if (sdkRoot !== undefined) {
    if (
      sha256(readFileSync(join(sdkRoot, "package.json"))) !==
        lock.cloudflareSdk.packageJsonSha256 ||
      sha256(readFileSync(join(sdkRoot, "index.mjs"))) !==
        lock.cloudflareSdk.indexSha256 ||
      sha256(
        readFileSync(join(sdkRoot, "resources/workers/scripts/scripts.mjs")),
      ) !== lock.cloudflareSdk.workersScriptsResourceSha256
    ) {
      throw new Error(
        "installed Cloudflare SDK does not match the fixed contract",
      );
    }
    const routes = sdkRoutes(sdkRoot);
    for (const required of [
      "/accounts/{account_id}/workers/scripts/{script_name}",
      "/accounts/{account_id}/workers/scripts/{script_name}/settings",
      "/accounts/{account_id}/workers/scripts/{script_name}/script-settings",
    ])
      if (!routes.has(required))
        throw new Error(
          `Cloudflare SDK route inventory is missing ${required}`,
        );
  }
}

function lockStatus(value) {
  return [
    "supported",
    "supported_with_deviation",
    "planned",
    "unsupported",
  ].includes(value);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  const value = (name) => {
    const index = args.indexOf(name);
    return index < 0 ? undefined : resolve(args[index + 1]);
  };
  if (command === "generate") {
    const openapiPath = value("--openapi");
    if (openapiPath === undefined)
      throw new Error("generate requires --openapi <pinned-openapi.json>");
    const lock = json(LOCK_PATH);
    const bytes = readFileSync(openapiPath);
    if (sha256(bytes) !== lock.sha256)
      throw new Error("official OpenAPI snapshot digest mismatch");
    const output = prettierJson(
      `${JSON.stringify(buildSubset(JSON.parse(bytes), json(MANIFEST_PATH), lock.revision, lock.sha256), null, 2)}\n`,
    );
    writeFileSync(SUBSET_PATH, output);
    const wranglerRoot = value("--wrangler-root");
    if (wranglerRoot === undefined)
      throw new Error(
        "generate requires --wrangler-root <wrangler-package-root>",
      );
    const capability = buildCapability(
      JSON.parse(output),
      json(MANIFEST_PATH),
      json(CAPABILITY_SOURCE_PATH),
      lock.wrangler.configSchemaSha256,
      json(join(wranglerRoot, "config-schema.json")),
    );
    writeFileSync(
      CAPABILITY_PATH,
      prettierJson(`${JSON.stringify(capability, null, 2)}\n`),
    );
    writeFileSync(
      EXTENSION_PATH,
      prettierJson(
        `${JSON.stringify(buildExtension(json(CAPABILITY_SOURCE_PATH)), null, 2)}\n`,
      ),
    );
    process.stdout.write(`${sha256(readFileSync(SUBSET_PATH))}\n`);
    return;
  }
  if (command !== "check")
    throw new Error(
      "usage: p6-contract.mjs generate|check [--openapi path] [--wrangler-root path] [--sdk-root path]",
    );
  validateCommitted({
    openapiPath: value("--openapi"),
    wranglerRoot: value("--wrangler-root"),
    sdkRoot: value("--sdk-root"),
  });
}

if (
  process.argv[1] !== undefined &&
  resolve(process.argv[1]) === fileURLToPath(import.meta.url)
)
  main();
