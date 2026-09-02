import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
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

function operationKey(value) {
  const separator = value.indexOf(" ");
  if (separator < 1) throw new Error(`invalid operation key: ${value}`);
  return [value.slice(0, separator).toLowerCase(), value.slice(separator + 1)];
}

function atPointer(document, pointer) {
  let value = document;
  for (const part of pointer.slice(2).split("/")) {
    value = value[part.replaceAll("~1", "/").replaceAll("~0", "~")];
    if (value === undefined) throw new Error(`missing OpenAPI reference: ${pointer}`);
  }
  return value;
}

function addPointer(output, pointer, value) {
  let target = output;
  const parts = pointer.slice(2).split("/").map(part => part.replaceAll("~1", "/").replaceAll("~0", "~"));
  for (const part of parts.slice(0, -1)) target = target[part] ??= {};
  target[parts.at(-1)] = value;
}

function collectRefs(value, refs) {
  if (Array.isArray(value)) {
    for (const item of value) collectRefs(item, refs);
    return;
  }
  if (value === null || typeof value !== "object") return;
  if (typeof value.$ref === "string" && value.$ref.startsWith("#/components/")) refs.add(value.$ref);
  for (const item of Object.values(value)) collectRefs(item, refs);
}

export function buildSubset(openapi, manifest, revision, sourceSha256) {
  const paths = {};
  const refs = new Set();
  const operationInventory = [];
  const seen = new Set();
  const deferred = new Map(manifest.deferredOperations.map(item => [item.operation, item]));
  const unsupported = new Map(manifest.unsupportedOperations.map(item => [item.operation, item]));
  const selectedOperations = [...manifest.operations, ...deferred.keys(), ...unsupported.keys()];
  for (const key of selectedOperations) {
    if (seen.has(key)) throw new Error(`duplicate selected operation: ${key}`);
    seen.add(key);
    const [method, path] = operationKey(key);
    const sourcePath = openapi.paths?.[path];
    const operation = sourcePath?.[method];
    if (operation === undefined) throw new Error(`selected operation is absent upstream: ${key}`);
    const selectedPath = paths[path] ??= {};
    if (sourcePath.parameters !== undefined) selectedPath.parameters = sourcePath.parameters;
    selectedPath[method] = operation;
    collectRefs(selectedPath, refs);
    operationInventory.push({
      id: key,
      method: method.toUpperCase(),
      path,
      operationId: operation.operationId,
      status: unsupported.has(key) ? "unsupported" : "planned",
      source: "cloudflare-openapi",
      ...(deferred.has(key) ? { stage: deferred.get(key).stage } : {}),
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
      description: "Mechanically selected from the pinned official Cloudflare OpenAPI snapshot; do not hand-edit.",
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
  if (content.some(value => value.startsWith("multipart/"))) return "multipart";
  if (content.some(value => value === "application/json" || value.endsWith("+json"))) return "json";
  return "raw";
}

export function buildCapability(subset, manifest, source, configSchemaSha256, configSchema) {
  const operations = new Map(subset["x-open-compute-operation-inventory"].map(item => [item.id, item]));
  const routes = manifest.operations.map(id => {
    const item = operations.get(id);
    const operation = subset.paths[item.path][item.method.toLowerCase()];
    return { ...item, requestMediaType: requestMediaType(operation) };
  });
  for (const item of manifest.deferredOperations) {
    const operation = operations.get(item.operation);
    const body = subset.paths[operation.path][operation.method.toLowerCase()];
    routes.push({ ...operation, status: item.status, source: "cloudflare-openapi", stage: item.stage,
      requestMediaType: requestMediaType(body) });
  }
  for (const item of manifest.unsupportedOperations) {
    const [method, path] = operationKey(item.operation);
    routes.push({ id: item.operation, method: method.toUpperCase(), path, status: item.status,
      source: "cloudflare-openapi", constraint: item.reason, requestMediaType: "none" });
  }
  for (const item of manifest.wranglerObservedOperations) {
    const [method, path] = operationKey(item.operation);
    routes.push({ id: item.operation, method: method.toUpperCase(), path, status: item.status,
      source: item.source, constraint: item.note, requestMediaType: "multipart" });
  }
  for (const id of source.managementApi.vendorRoutes) {
    const [method, path] = operationKey(id);
    routes.push({ id, method: method.toUpperCase(), path, status: "planned", source: "open-compute-extension",
      requestMediaType: method === "get" ? "none" : "json" });
  }
  const topFields = Object.keys(configSchema.definitions?.RawConfig?.properties ?? {});
  if (topFields.length === 0) throw new Error("Wrangler RawConfig field inventory is empty");
  const statusByField = new Map();
  for (const id of source.wrangler.plannedFields) statusByField.set(id, { status: "planned", source: "wrangler-config-schema" });
  for (const [id, stage] of Object.entries(source.wrangler.deferredFields))
    statusByField.set(id, { status: "planned", source: "wrangler-config-schema", stage });
  for (const [id, stage] of Object.entries(source.wrangler.unsupportedDeferredFields))
    statusByField.set(id, { status: "unsupported", source: "wrangler-config-schema", stage });
  for (const id of topFields) if (!statusByField.has(id))
    statusByField.set(id, { status: "unsupported", source: "wrangler-config-schema" });
  for (const id of statusByField.keys()) if (!topFields.includes(id)) throw new Error(`unknown Wrangler top-level field: ${id}`);
  const fields = topFields.map(id => ({ id, ...statusByField.get(id) }));
  fields.push(
    { id: "observability.logs", status: "planned", source: "wrangler-config-schema", stage: "P7" },
    { id: "observability.traces", status: "unsupported", source: "wrangler-config-schema", stage: "P7" },
    { id: "limits.cpu_ms", status: "unsupported", source: "wrangler-config-schema", stage: "P8" },
    { id: "limits.subrequests", status: "unsupported", source: "wrangler-config-schema", stage: "P8" },
    { id: "usage_model", status: "unsupported", source: "pinned-schema-absence", stage: "P8",
      constraint: "wrangler@4.127.1 config-schema.json has no usage_model property" },
    { id: "worker_loaders[].binding", status: "unsupported", source: "wrangler-config-schema", stage: "P9" },
  );
  const bindings = [
    ...source.wrangler.plannedBindings.map(id => ({ id, status: "planned", source: "wrangler-multipart" })),
    ...source.wrangler.unsupportedBindings.map(id => ({ id, status: "unsupported", source: "wrangler-multipart",
      ...(id === "worker_loader" ? { stage: "P9" } : {}) })),
  ];
  const commands = [
    ...source.wrangler.commands.map(id => ({ id, status: "planned", source: "wrangler-cli" })),
    ...Object.entries(source.wrangler.deferredCommands).map(([id, stage]) =>
      ({ id, status: "planned", source: "wrangler-cli", stage })),
    ...source.wrangler.unsupportedCommands.map(id => ({ id, status: "unsupported", source: "wrangler-cli" })),
  ];
  return {
    schemaVersion: 1,
    managementApi: {
      root: source.managementApi.root,
      routes,
      legacyRoutes: source.managementApi.legacyRoutes.map(id =>
        ({ id, status: "unsupported", source: "day1-negative-route-inventory" })),
    },
    wrangler: { version: "4.127.1", configSchemaSha256, fields, bindings, commands },
  };
}

export function buildExtension(source) {
  const paths = {};
  for (const id of source.managementApi.vendorRoutes) {
    const [method, path] = operationKey(id);
    const parameters = [...path.matchAll(/\{([^}]+)\}/g)].map(match => ({
      name: match[1], in: "path", required: true, schema: { type: "string", minLength: 1 },
    }));
    paths[path] ??= {};
    paths[path][method] = {
      operationId: `open-compute-${method}-${path.replaceAll(/[^a-zA-Z0-9]+/g, "-").replaceAll(/^-|-$/g, "")}`,
      "x-open-compute-capability-status": "planned",
      parameters,
      ...(method === "post" ? { requestBody: { required: true, content: {
        "application/json": { schema: { type: "object", additionalProperties: false } },
      } } } : {}),
      responses: {
        "200": { description: "Successful vendor extension response", content: {
          "application/json": { schema: { $ref: "#/components/schemas/Envelope" } },
        } },
        default: { description: "Cloudflare-style or vendor-reserved error", content: {
          "application/json": { schema: { $ref: "#/components/schemas/Envelope" } },
        } },
      },
    };
  }
  return {
    openapi: "3.1.0",
    info: { title: "open-compute Cloudflare v4 extension", version: "0.1.0",
      description: "Source schema for vendor-only routes. Planned operations are not an implementation claim." },
    servers: [{ url: "/client/v4" }],
    paths,
    components: { schemas: {
      Error: { type: "object", additionalProperties: false, required: ["code", "message"], properties: {
        code: { type: "integer", minimum: 9_100_000, maximum: 9_199_999 }, message: { type: "string" },
        source: { type: "object", additionalProperties: false, properties: { pointer: { type: "string" } } },
      } },
      Envelope: { type: "object", additionalProperties: false, required: ["success", "result", "errors", "messages"], properties: {
        success: { type: "boolean" }, result: {}, errors: { type: "array", items: { $ref: "#/components/schemas/Error" } },
        messages: { type: "array", items: {} },
      } },
    } },
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
    for (const match of source.matchAll(/`(\/accounts\/\$\{account_id\}[^`]*)`/g)) {
      routes.add(match[1]
        .replaceAll("${account_id}", "{account_id}")
        .replaceAll(/\$\{(?:scriptName|script_name)\}/g, "{script_name}")
        .replaceAll("${versionID}", "{version_id}")
        .replaceAll("${deploymentID}", "{deployment_id}"));
    }
  }
  return routes;
}

export function validateCommitted({ openapiPath, wranglerRoot, sdkRoot } = {}) {
  const lock = json(LOCK_PATH);
  const manifest = json(MANIFEST_PATH);
  if (sha256(readFileSync(MANIFEST_PATH)) !== lock.subsetManifestSha256
      || sha256(readFileSync(EXTENSION_PATH)) !== lock.extensionSha256
      || sha256(readFileSync(CAPABILITY_PATH)) !== lock.capabilitySha256
      || sha256(readFileSync(join(ROOT, "openapi/capability-manifest.schema.json"))) !== lock.capabilitySchemaSha256) {
    throw new Error("P6 contract authority digest drift");
  }
  const subsetBytes = readFileSync(SUBSET_PATH);
  if (lock.subsetSha256 !== sha256(subsetBytes)) throw new Error("generated OpenAPI subset digest drift");
  const subset = JSON.parse(subsetBytes);
  const inventory = subset["x-open-compute-operation-inventory"];
  const selectedIds = [...manifest.operations, ...manifest.deferredOperations.map(item => item.operation),
    ...manifest.unsupportedOperations.map(item => item.operation)];
  if (inventory.length !== selectedIds.length) throw new Error("subset operation inventory is incomplete");
  const ids = inventory.map(item => item.id);
  if (new Set(ids).size !== ids.length || ids.join("\0") !== selectedIds.join("\0")) {
    throw new Error("subset operations differ from the selection manifest");
  }
  const capability = json(CAPABILITY_PATH);
  const extension = json(EXTENSION_PATH);
  const expectedVendorPaths = new Set(json(CAPABILITY_SOURCE_PATH).managementApi.vendorRoutes.map(id => operationKey(id)[1]));
  if (Object.keys(extension.paths).length !== expectedVendorPaths.size
      || Object.keys(extension.paths).some(path => !expectedVendorPaths.has(path))) {
    throw new Error("vendor extension schema route inventory drift");
  }
  const routeIds = capability.managementApi.routes.map(item => item.id);
  for (const id of [...manifest.operations, ...manifest.deferredOperations.map(item => item.operation),
    ...manifest.unsupportedOperations.map(item => item.operation), ...manifest.wranglerObservedOperations.map(item => item.operation)]) {
    if (!routeIds.includes(id)) throw new Error(`capability route inventory is missing ${id}`);
  }
  for (const item of capability.managementApi.routes) {
    if (!lockStatus(item.status)) throw new Error(`invalid route status: ${item.id}`);
  }
  for (const collection of [capability.wrangler.fields, capability.wrangler.bindings, capability.wrangler.commands]) {
    if (new Set(collection.map(item => item.id)).size !== collection.length) throw new Error("duplicate capability item");
    for (const item of collection) if (!lockStatus(item.status)) throw new Error(`invalid capability status: ${item.id}`);
  }
  if (capability.wrangler.version !== lock.wrangler.version
      || capability.wrangler.configSchemaSha256 !== lock.wrangler.configSchemaSha256) {
    throw new Error("Wrangler capability identity differs from the fixed lock");
  }
  if (openapiPath !== undefined) {
    const bytes = readFileSync(openapiPath);
    if (sha256(bytes) !== lock.sha256) throw new Error("official OpenAPI snapshot digest mismatch");
    const rebuilt = `${JSON.stringify(buildSubset(JSON.parse(bytes), manifest, lock.revision, lock.sha256), null, 2)}\n`;
    if (!Buffer.from(rebuilt).equals(subsetBytes)) throw new Error("committed OpenAPI subset is not reproducible");
  }
  if (wranglerRoot !== undefined) {
    if (sha256(readFileSync(join(wranglerRoot, "config-schema.json"))) !== lock.wrangler.configSchemaSha256
        || sha256(readFileSync(join(wranglerRoot, "wrangler-dist/cli.js"))) !== lock.wrangler.cliSha256) {
      throw new Error("installed Wrangler does not match the fixed contract");
    }
    const source = readFileSync(join(wranglerRoot, "wrangler-dist/cli.js"), "utf8");
    for (const required of ["bindings_inherit: \"strict\"", "/script-settings`,", "/workers/assets/upload/${manifestEntry[1].hash}"])
      if (!source.includes(required)) throw new Error(`Wrangler trace authority is missing ${required}`);
    const expected = `${JSON.stringify(buildCapability(subset, manifest, json(CAPABILITY_SOURCE_PATH),
      lock.wrangler.configSchemaSha256, json(join(wranglerRoot, "config-schema.json"))), null, 2)}\n`;
    if (expected !== readFileSync(CAPABILITY_PATH, "utf8")) throw new Error("P6 capability projection is not reproducible");
  }
  if (sdkRoot !== undefined) {
    if (sha256(readFileSync(join(sdkRoot, "package.json"))) !== lock.cloudflareSdk.packageJsonSha256
        || sha256(readFileSync(join(sdkRoot, "index.mjs"))) !== lock.cloudflareSdk.indexSha256
        || sha256(readFileSync(join(sdkRoot, "resources/workers/scripts/scripts.mjs"))) !== lock.cloudflareSdk.workersScriptsResourceSha256) {
      throw new Error("installed Cloudflare SDK does not match the fixed contract");
    }
    const routes = sdkRoutes(sdkRoot);
    for (const required of [
      "/accounts/{account_id}/workers/scripts/{script_name}",
      "/accounts/{account_id}/workers/scripts/{script_name}/settings",
      "/accounts/{account_id}/workers/scripts/{script_name}/script-settings",
    ]) if (!routes.has(required)) throw new Error(`Cloudflare SDK route inventory is missing ${required}`);
  }
}

function lockStatus(value) {
  return ["supported", "supported_with_deviation", "planned", "unsupported"].includes(value);
}

function main() {
  const [command, ...args] = process.argv.slice(2);
  const value = name => {
    const index = args.indexOf(name);
    return index < 0 ? undefined : resolve(args[index + 1]);
  };
  if (command === "generate") {
    const openapiPath = value("--openapi");
    if (openapiPath === undefined) throw new Error("generate requires --openapi <pinned-openapi.json>");
    const lock = json(LOCK_PATH);
    const bytes = readFileSync(openapiPath);
    if (sha256(bytes) !== lock.sha256) throw new Error("official OpenAPI snapshot digest mismatch");
    const output = `${JSON.stringify(buildSubset(JSON.parse(bytes), json(MANIFEST_PATH), lock.revision, lock.sha256), null, 2)}\n`;
    writeFileSync(SUBSET_PATH, output);
    const wranglerRoot = value("--wrangler-root");
    if (wranglerRoot === undefined) throw new Error("generate requires --wrangler-root <wrangler-package-root>");
    const capability = buildCapability(JSON.parse(output), json(MANIFEST_PATH), json(CAPABILITY_SOURCE_PATH),
      lock.wrangler.configSchemaSha256, json(join(wranglerRoot, "config-schema.json")));
    writeFileSync(CAPABILITY_PATH, `${JSON.stringify(capability, null, 2)}\n`);
    writeFileSync(EXTENSION_PATH, `${JSON.stringify(buildExtension(json(CAPABILITY_SOURCE_PATH)), null, 2)}\n`);
    process.stdout.write(`${sha256(output)}\n`);
    return;
  }
  if (command !== "check") throw new Error("usage: p6-contract.mjs generate|check [--openapi path] [--wrangler-root path] [--sdk-root path]");
  validateCommitted({
    openapiPath: value("--openapi"),
    wranglerRoot: value("--wrangler-root"),
    sdkRoot: value("--sdk-root"),
  });
}

if (process.argv[1] !== undefined && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
