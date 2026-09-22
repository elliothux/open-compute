import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { scanOfficialPackage } from "./sdk-scan.ts";

const PACKAGE_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = resolve(PACKAGE_ROOT, "../..");
const CHECK = process.argv.includes("--check");

interface LockAuthority {
  schemaVersion: number;
  revision: string;
  sha256: string;
  wrangler: { version: string };
  cloudflareSdk: {
    version: string;
    npmIntegrity: string;
    npmShasum: string;
    packageSha256: string;
    packageJsonSha256: string;
    indexSha256: string;
    workersScriptsResourceSha256: string;
  };
  subsetSha256: string;
  subsetManifestSha256: string;
  extensionSha256: string;
  observedStandardSha256: string;
}

interface ManifestEntry {
  operation: string;
  reason: string;
}

interface SubsetInventoryEntry {
  id: string;
  method: string;
  path: string;
  operationId: string;
  status: string;
  operationSha256: string;
  deviations?: string[];
}

interface Schema {
  $ref?: string;
  const?: string | number | boolean;
  enum?: Array<string | number | boolean>;
  type?: string | string[];
  required?: string[];
  properties?: Record<string, Schema>;
  items?: Schema;
  additionalProperties?: Schema | boolean;
}

interface VendorOperation {
  operationId: string;
  "x-open-compute-sdk-method": string;
  parameters?: Array<{ name: string }>;
  requestBody?: unknown;
  responses: Record<
    string,
    { content: { "application/json": { schema: Schema } } }
  >;
}

interface MappedOperation {
  operation: string;
  method: string;
  path: string;
  operationId: string;
  status: "supported" | "supported_with_deviation";
  deviations: string[];
  operationSha256: string;
  node: string;
  officialMethod: string;
  delegateModule: string;
  delegateClass: string;
  types: string[];
}

function sha256(bytes: Buffer | string): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function readJson(path: string): unknown {
  return JSON.parse(readFileSync(path, "utf8"));
}

function requireStringArray(value: unknown, field: string): string[] {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string"))
    throw new Error(`authority field ${field} must be a string array`);
  return value as string[];
}

function pascalCase(value: string): string {
  return value
    .split("-")
    .map((part) =>
      part === "" ? "" : part.charAt(0).toUpperCase() + part.slice(1),
    )
    .join("");
}

function camelCase(value: string): string {
  const pascal = pascalCase(value);
  return pascal === ""
    ? pascal
    : pascal.charAt(0).toLowerCase() + pascal.slice(1);
}

function lowerCamelParameter(snake: string): string {
  const camel = camelCase(snake.replaceAll("_", "-"));
  if (!/^[a-z][a-zA-Z0-9]*$/.test(camel))
    throw new Error(`unsupported vendor path parameter: ${snake}`);
  return camel;
}

function typeForSchema(schema: Schema | boolean, indent = ""): string {
  if (schema === true) return "unknown";
  if (schema === false) return "never";
  if (schema.$ref !== undefined)
    return schema.$ref.split("/").at(-1) ?? "unknown";
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (schema.enum !== undefined)
    return schema.enum.map((value) => JSON.stringify(value)).join(" | ");
  const schemaType = schema.type;
  if (Array.isArray(schemaType)) {
    return schemaType
      .map((type) => typeForSchema({ ...schema, type }, indent))
      .join(" | ");
  }
  if (schemaType === undefined)
    throw new Error(
      `unsupported vendor schema node ${JSON.stringify(schema).slice(0, 120)}`,
    );
  const type = schemaType;
  if (type === "string") return "string";
  if (type === "integer" || type === "number") return "number";
  if (type === "boolean") return "boolean";
  if (type === "null") return "null";
  if (type === "array")
    return `readonly ${typeForSchema(schema.items ?? {}, indent)}[]`;
  if (type === "object") {
    const required = new Set(schema.required ?? []);
    const entries = Object.entries(schema.properties ?? {});
    if (entries.length === 0 && schema.additionalProperties !== undefined) {
      return `Record<string, ${typeForSchema(schema.additionalProperties, indent)}>`;
    }
    if (entries.length === 0) return "Record<string, never>";
    const next = `${indent}  `;
    const fields = entries
      .map(
        ([name, value]) =>
          `${next}readonly ${name}${required.has(name) ? "" : "?"}: ${typeForSchema(value, next)};`,
      )
      .join("\n");
    return `{\n${fields}\n${indent}}`;
  }
  throw new Error(
    `unsupported vendor schema node ${JSON.stringify(schema).slice(0, 120)}`,
  );
}

interface Authority {
  lock: LockAuthority;
  subset: {
    openapi: string;
    "x-open-compute-upstream": { revision: string; sha256: string };
    "x-open-compute-operation-inventory": SubsetInventoryEntry[];
    paths: Record<string, Record<string, unknown>>;
    components: { schemas: Record<string, unknown> };
  };
  manifest: Record<string, unknown>;
  extension: {
    paths: Record<string, Record<string, VendorOperation>>;
    components: { schemas: Record<string, Schema> };
  };
  observed: {
    "x-open-compute-evidence": { schemaSha256: Record<string, string> };
    paths: Record<
      string,
      Record<
        string,
        { operationId: string; "x-open-compute-sdk-method": string }
      >
    >;
    components: {
      parameters: Record<string, unknown>;
      schemas: Record<string, unknown>;
      requestBodies: Record<string, unknown>;
      responses: Record<string, unknown>;
    };
  };
}

function loadAuthority(): Authority {
  const lockPath = resolve(
    REPO_ROOT,
    "openapi/upstream/cloudflare-openapi.lock.json",
  );
  const subsetPath = resolve(REPO_ROOT, "openapi/cloudflare-v4-subset.json");
  const manifestPath = resolve(
    REPO_ROOT,
    "openapi/cloudflare-subset-manifest.json",
  );
  const extensionPath = resolve(
    REPO_ROOT,
    "openapi/open-compute-extension.json",
  );
  const observedPath = resolve(
    REPO_ROOT,
    "openapi/cloudflare-observed-standard.json",
  );
  const lock = readJson(lockPath) as LockAuthority;
  if (lock.schemaVersion !== 1)
    throw new Error("unsupported lock schema version");
  const digests: Array<[string, string, string]> = [
    [subsetPath, "subsetSha256", lock.subsetSha256],
    [manifestPath, "subsetManifestSha256", lock.subsetManifestSha256],
    [extensionPath, "extensionSha256", lock.extensionSha256],
    [observedPath, "observedStandardSha256", lock.observedStandardSha256],
  ];
  for (const [path, field, expected] of digests) {
    if (sha256(readFileSync(path)) !== expected)
      throw new Error(`authority digest drift on ${field}`);
  }
  const sdkRoot = resolve(PACKAGE_ROOT, "node_modules/cloudflare");
  const sdkChecks: Array<[string, string]> = [
    ["package.json", lock.cloudflareSdk.packageJsonSha256],
    ["index.mjs", lock.cloudflareSdk.indexSha256],
    [
      "resources/workers/scripts/scripts.mjs",
      lock.cloudflareSdk.workersScriptsResourceSha256,
    ],
  ];
  for (const [file, expected] of sdkChecks) {
    if (sha256(readFileSync(resolve(sdkRoot, file))) !== expected)
      throw new Error(
        `installed Cloudflare SDK does not match the fixed lock identity (${file})`,
      );
  }
  const observed = readJson(observedPath) as Authority["observed"];
  const schemaNames = Object.keys(observed.components.schemas).sort();
  if (
    JSON.stringify(
      Object.keys(observed["x-open-compute-evidence"].schemaSha256).sort(),
    ) !== JSON.stringify(schemaNames)
  )
    throw new Error("observed-standard schema digest inventory drift");
  for (const name of schemaNames) {
    if (
      sha256(JSON.stringify(observed.components.schemas[name])) !==
      observed["x-open-compute-evidence"].schemaSha256[name]
    )
      throw new Error(`observed-standard schema digest drift: ${name}`);
  }
  return {
    lock,
    subset: readJson(subsetPath) as Authority["subset"],
    manifest: readJson(manifestPath) as Authority["manifest"],
    extension: readJson(extensionPath) as Authority["extension"],
    observed,
  };
}

interface RouteCandidate {
  id: string;
  module: string;
  className: string;
  method: string;
  key: string[];
}

function buildRouteIndex(
  scan: Awaited<ReturnType<typeof scanOfficialPackage>>,
): Map<string, RouteCandidate[]> {
  const index = new Map<string, RouteCandidate[]>();
  for (const method of scan.methods.values()) {
    const route = `${method.httpMethod} ${method.pathTemplate}`;
    const candidates = index.get(route) ?? [];
    candidates.push({
      id: `${method.module}::${method.className}::${method.method}`,
      module: method.module,
      className: method.className,
      method: method.method,
      key: method.key,
    });
    index.set(route, candidates);
  }
  return index;
}

function mapOperations(
  authority: Authority,
  scan: Awaited<ReturnType<typeof scanOfficialPackage>>,
  routeIndex: Map<string, RouteCandidate[]>,
): { mapped: MappedOperation[]; excluded: ManifestEntry[] } {
  const excludedOperations = new Set(
    requireStringArray(
      (
        authority.manifest.sdkExcludedOperations as ManifestEntry[] | undefined
      )?.map((entry) => entry.operation) ?? [],
      "sdkExcludedOperations",
    ),
  );
  const excludedReasons = new Map<string, string>();
  for (const entry of (authority.manifest.sdkExcludedOperations as
    ManifestEntry[] | undefined) ?? []) {
    excludedReasons.set(entry.operation, entry.reason);
  }
  const deviations = new Map<string, string[]>();
  for (const entry of (authority.manifest.supportedWithDeviationOperations as
    Array<{ operation: string; deviations: string[] }> | undefined) ?? []) {
    deviations.set(entry.operation, entry.deviations);
  }
  const mapped: MappedOperation[] = [];
  const excluded: ManifestEntry[] = [];
  const claimedMethods = new Set<string>();
  for (const entry of authority.subset["x-open-compute-operation-inventory"]) {
    if (
      entry.status !== "supported" &&
      entry.status !== "supported_with_deviation"
    )
      continue;
    if (excludedOperations.has(entry.id)) {
      excluded.push({
        operation: entry.id,
        reason:
          excludedReasons.get(entry.id) ??
          "excluded by the selection manifest without a recorded reason",
      });
      continue;
    }
    const normalized = entry.path.replaceAll(/\{[^}]*\}/g, "{}");
    const candidates = routeIndex.get(`${entry.method} ${normalized}`);
    if (candidates === undefined || candidates.length === 0) {
      throw new Error(
        `no official SDK method implements ${entry.id}; keep the operation out of the SDK surface via the selection manifest or upgrade the pinned SDK`,
      );
    }
    const ranked = [...candidates].sort((left, right) => {
      if (left.key.length !== right.key.length)
        return left.key.length - right.key.length;
      const leftBase = left.className.startsWith("Base") ? 0 : 1;
      const rightBase = right.className.startsWith("Base") ? 0 : 1;
      return leftBase - rightBase;
    });
    const chosen = ranked[0];
    const rival = ranked[1];
    if (chosen === undefined) throw new Error(`no candidate for ${entry.id}`);
    const baseRank = (candidate: RouteCandidate): number =>
      candidate.className.startsWith("Base") ? 0 : 1;
    if (
      rival !== undefined &&
      rival.key.length === chosen.key.length &&
      baseRank(rival) === baseRank(chosen)
    ) {
      throw new Error(
        `ambiguous official SDK methods for ${entry.id}: ${candidates
          .map((candidate) => candidate.id)
          .join(", ")}`,
      );
    }
    const declaration = scan.declarations.get(chosen.id);
    if (declaration === undefined) {
      throw new Error(
        `official SDK declarations are missing ${chosen.id} for ${entry.id}`,
      );
    }
    if (declaration.overloads > 1) {
      throw new Error(
        `official SDK method ${chosen.id} is overloaded; upgrade the scanner before mapping ${entry.id}`,
      );
    }
    if (claimedMethods.has(chosen.id)) {
      throw new Error(
        `official SDK method ${chosen.id} claims more than one selected operation`,
      );
    }
    claimedMethods.add(chosen.id);
    mapped.push({
      operation: entry.id,
      method: entry.method,
      path: entry.path,
      operationId: entry.operationId,
      status:
        entry.status === "supported" ? "supported" : "supported_with_deviation",
      deviations: deviations.get(entry.id) ?? [],
      operationSha256: entry.operationSha256,
      node: chosen.key.join("."),
      officialMethod: chosen.method,
      delegateModule: chosen.module.replace(/\.mjs$/, ""),
      delegateClass: chosen.className,
      types: declaration.typeNames,
    });
  }
  return { mapped, excluded };
}

interface TypeReExport {
  /** Official exported type name. */
  name: string;
  /** Public alias when the official name collides across modules. */
  as?: string;
}

function moduleAliasStem(module: string): string {
  return module
    .replace(/^resources\//, "")
    .split("/")
    .map((segment) => segment.replace(/\.mjs$/, ""))
    .map(pascalCase)
    .join("");
}

function planTypeReExports(
  mapped: MappedOperation[],
  scan: Awaited<ReturnType<typeof scanOfficialPackage>>,
): Map<string, TypeReExport[]> {
  const modulesByName = new Map<string, Set<string>>();
  for (const operation of [...mapped].sort((left, right) =>
    left.operation.localeCompare(right.operation),
  )) {
    const module = `${operation.delegateModule}.d.ts`;
    const localExports = scan.exportedTypes.get(module);
    if (localExports === undefined)
      throw new Error(`official SDK module has no declarations: ${module}`);
    for (const name of operation.types) {
      if (!localExports.has(name)) {
        throw new Error(
          `official SDK type ${name} is not exported from ${module} for ${operation.operation}`,
        );
      }
      const modules = modulesByName.get(name) ?? new Set<string>();
      modules.add(operation.delegateModule);
      modulesByName.set(name, modules);
    }
  }
  const grouped = new Map<string, TypeReExport[]>();
  for (const [name, modules] of modulesByName) {
    for (const module of modules) {
      const names = grouped.get(module) ?? [];
      if (modules.size > 1)
        names.push({ name, as: `${moduleAliasStem(module)}${name}` });
      else names.push({ name });
      grouped.set(module, names);
    }
  }
  for (const [module, names] of grouped) {
    grouped.set(
      module,
      [...names].sort((left, right) =>
        (left.as ?? left.name).localeCompare(right.as ?? right.name),
      ),
    );
  }
  return grouped;
}

interface TreeNode {
  children: Map<string, TreeNode>;
  methods: Array<{
    operation: string;
    officialMethod: string;
    variable: string;
  }>;
  variable?: string;
}

function buildTree(mapped: MappedOperation[]): TreeNode {
  const root: TreeNode = { children: new Map(), methods: [] };
  const nodeVariables = new Map<string, string>();
  for (const operation of mapped) {
    const segments = operation.node.split(".");
    let current = root;
    const variable = `${operation.node.split(".").map(camelCase).join("")}`;
    const existing = nodeVariables.get(operation.node);
    if (existing !== undefined && existing !== variable)
      throw new Error(`conflicting facade variable for ${operation.node}`);
    nodeVariables.set(operation.node, variable);
    for (const segment of segments) {
      let child = current.children.get(segment);
      if (child === undefined) {
        child = { children: new Map(), methods: [] };
        current.children.set(segment, child);
      }
      current = child;
    }
    current.variable = variable;
    current.methods.push({
      operation: operation.operation,
      officialMethod: operation.officialMethod,
      variable,
    });
  }
  return root;
}

function nodeInterfaceName(node: string): string {
  return `OpenCompute${node.split(".").map(pascalCase).join("")}Node`;
}

function renderNodeInterfaces(
  node: TreeNode,
  path: string[],
  delegates: Map<string, NodeDelegate>,
  aliases: Map<string, string>,
): string {
  const lines: string[] = [];
  const interfaceName = nodeInterfaceName(path.join("."));
  const members: string[] = [];
  for (const [name, child] of [...node.children].sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    lines.push(
      renderNodeInterfaces(child, [...path, name], delegates, aliases),
    );
    members.push(
      `  readonly ${name}: ${nodeInterfaceName([...path, name].join("."))};`,
    );
  }
  const nodeKey = path.join(".");
  const delegate = delegates.get(nodeKey);
  if (delegate !== undefined) {
    const alias = aliases.get(nodeKey);
    if (alias === undefined)
      throw new Error(`missing import alias for ${nodeKey}`);
    for (const method of [...node.methods].sort((a, b) =>
      a.officialMethod.localeCompare(b.officialMethod),
    )) {
      if (
        method.operation ===
        "POST /accounts/{account_id}/workers/scripts/{script_name}/versions"
      ) {
        members.push(
          `  readonly create: (scriptName: string, params: OpenComputeWorkerVersionCreateParams, options?: OpenComputeRequestOptions) => ReturnType<${alias}["create"]>;`,
        );
      } else if (
        method.operation ===
        "PUT /accounts/{account_id}/workers/scripts/{script_name}"
      ) {
        members.push(
          `  readonly update: (scriptName: string, params: OpenComputeWorkerScriptUpdateParams, options?: OpenComputeRequestOptions) => ReturnType<${alias}["update"]>;`,
        );
      } else if (
        method.operation === "POST /accounts/{account_id}/workers/assets/upload"
      ) {
        members.push(
          `  readonly create: (params: OpenComputeAssetsUploadCreateParams, options?: OpenComputeRequestOptions) => ReturnType<${alias}["create"]>;`,
        );
      } else {
        members.push(
          `  readonly ${method.officialMethod}: ${alias}["${method.officialMethod}"];`,
        );
      }
    }
  }
  lines.unshift(
    `export interface ${interfaceName} {\n${members.join("\n")}\n}`,
  );
  return lines.join("\n\n");
}

function renderRuntimeTree(node: TreeNode, indent: string): string {
  const inner = `${indent}  `;
  const lines: string[] = [];
  for (const [name, child] of [...node.children].sort(([a], [b]) =>
    a.localeCompare(b),
  )) {
    const childBody = renderRuntimeTree(child, inner);
    lines.push(`${inner}${name}: {\n${childBody}\n${inner}},`);
  }
  for (const method of [...node.methods].sort((a, b) =>
    a.officialMethod.localeCompare(b.officialMethod),
  )) {
    if (
      method.operation ===
      "POST /accounts/{account_id}/workers/scripts/{script_name}/versions"
    ) {
      lines.push(
        `${inner}create: (scriptName, params, options) => ${method.variable}.create(scriptName, params as VersionCreateParams, options),`,
      );
    } else if (
      method.operation ===
      "PUT /accounts/{account_id}/workers/scripts/{script_name}"
    ) {
      lines.push(
        `${inner}update: (scriptName, params, options) => ${method.variable}.update(scriptName, params as ScriptUpdateParams, params.files?.length ? scriptUploadOptions(options) : options),`,
      );
    } else if (
      method.operation === "POST /accounts/{account_id}/workers/assets/upload"
    ) {
      lines.push(
        `${inner}create: (params, options) => ${method.variable}.create(params as UploadCreateParams, options),`,
      );
    } else {
      lines.push(
        `${inner}${method.officialMethod}: ${method.variable}.${method.officialMethod}.bind(${method.variable}),`,
      );
    }
  }
  return lines.join("\n");
}

interface VendorMethod {
  tree: string[];
  verb: string;
  template: string;
  /** OpenAPI-style path with `{param}` placeholders. */
  openapiPath: string;
  arguments: string;
  resultType: string;
  hasBody: boolean;
  bodyType?: string;
}

function collectVendorMethods(authority: Authority): VendorMethod[] {
  const methods: VendorMethod[] = [];
  const schemas = authority.extension.components.schemas;
  const seen = new Set<string>();
  for (const [path, pathItem] of Object.entries(authority.extension.paths)) {
    for (const [verb, operation] of Object.entries(pathItem)) {
      if (verb === "parameters") continue;
      if (
        verb !== "get" &&
        verb !== "post" &&
        verb !== "put" &&
        verb !== "delete"
      )
        throw new Error(
          `unsupported vendor verb ${verb} on ${operation.operationId}`,
        );
      const tree = operation["x-open-compute-sdk-method"].split(".");
      const joined = tree.join(".");
      if (seen.has(joined))
        throw new Error(`duplicate vendor method ${joined}`);
      seen.add(joined);
      const success = Object.entries(operation.responses).filter(([status]) =>
        status.startsWith("2"),
      );
      if (success.length === 0)
        throw new Error(
          `vendor operation ${operation.operationId} has no success schema`,
        );
      const schemaRefs = new Set(
        success.map(([, response]) =>
          response.content["application/json"].schema.$ref?.split("/").at(-1),
        ),
      );
      if (schemaRefs.size !== 1)
        throw new Error(
          `vendor operation ${operation.operationId} must share one success schema across status codes`,
        );
      const envelopeName = [...schemaRefs][0];
      if (envelopeName === undefined)
        throw new Error(
          `vendor operation ${operation.operationId} has no envelope ref`,
        );
      const envelope = schemas[envelopeName];
      if (envelope === undefined)
        throw new Error(`vendor envelope ${envelopeName} is missing`);
      if (
        typeof envelope === "boolean" ||
        envelope.properties?.result === undefined
      )
        throw new Error(`vendor envelope ${envelopeName} has no result`);
      const result = envelope.properties.result;
      if (result === undefined)
        throw new Error(`vendor envelope ${envelopeName} has no result schema`);
      const resultName = result.$ref?.split("/").at(-1);
      const resultType = resultName ?? typeForSchema(result);
      const hasBody = operation.requestBody !== undefined;
      const bodySchema = hasBody
        ? (
            operation.requestBody as {
              content: Record<string, { schema: Schema }>;
            }
          ).content["application/json"]?.schema
        : undefined;
      const bodyType = bodySchema?.$ref?.split("/").at(-1);
      if (hasBody && bodyType === undefined)
        throw new Error(
          `vendor operation ${operation.operationId} must use a named JSON request schema`,
        );
      const parameters = (operation.parameters ?? []).map((parameter) => ({
        argument: lowerCamelParameter(parameter.name),
      }));
      const argumentList = [
        ...parameters.map((parameter) => `${parameter.argument}: string`),
        ...(bodyType === undefined ? [] : [`body: ${bodyType}`]),
        `options?: OpenComputeRequestOptions`,
      ].join(", ");
      const template = path
        .split("/")
        .map((segment) =>
          segment.startsWith("{") && segment.endsWith("}")
            ? `\${segment(${lowerCamelParameter(segment.slice(1, -1))})}`
            : segment,
        )
        .join("/");
      methods.push({
        tree,
        verb,
        template,
        openapiPath: path,
        arguments: argumentList,
        resultType,
        hasBody,
        ...(bodyType === undefined ? {} : { bodyType }),
      });
    }
  }
  return methods.sort((left, right) =>
    left.tree.join(".").localeCompare(right.tree.join(".")),
  );
}

function renderVendorTree(methods: VendorMethod[], indent: string): string {
  interface VendorNode {
    children: Map<string, VendorNode>;
    method?: VendorMethod;
  }
  const root: VendorNode = { children: new Map() };
  for (const method of methods) {
    let current = root;
    for (const segment of method.tree) {
      let child = current.children.get(segment);
      if (child === undefined) {
        child = { children: new Map() };
        current.children.set(segment, child);
      }
      current = child;
    }
    if (current.method !== undefined)
      throw new Error(
        `duplicate vendor method tree at ${method.tree.join(".")}`,
      );
    current.method = method;
  }
  const renderMethod = (method: VendorMethod, depth: string): string => {
    const withBody = method.hasBody;
    const call = withBody
      ? `transport.${method.verb}<V4Envelope<${method.resultType}>>(
${depth}  \`${method.template}\`,
${depth}  { ...options, body },
${depth})`
      : `transport.${method.verb}<V4Envelope<${method.resultType}>>(
${depth}  \`${method.template}\`,
${depth}  options,
${depth})`;
    return `(${method.arguments}): APIPromise<${method.resultType}> =>
${depth}  ${call}._thenUnwrap((envelope) => envelope.result),`;
  };
  const render = (node: VendorNode, depth: string): string => {
    const lines: string[] = [];
    for (const [name, child] of [...node.children].sort(([a], [b]) =>
      a.localeCompare(b),
    )) {
      if (child.children.size === 0 && child.method !== undefined) {
        lines.push(
          `${depth}${name}: ${renderMethod(child.method, `${depth}  `)}`,
        );
      } else {
        lines.push(
          `${depth}${name}: {\n${render(child, `${depth}  `)}\n${depth}},`,
        );
      }
    }
    return lines.join("\n");
  };
  return render(root, indent);
}

function vendorReachableTypes(authority: Authority): Array<[string, string]> {
  const schemas = authority.extension.components.schemas;
  const collectRefs = (schema: Schema | boolean, out: Set<string>): void => {
    if (typeof schema === "boolean") return;
    if (schema.$ref !== undefined) {
      const name = schema.$ref.split("/").at(-1);
      if (name !== undefined) out.add(name);
    }
    for (const value of Object.values(schema.properties ?? {}))
      collectRefs(value, out);
    if (schema.items !== undefined) collectRefs(schema.items, out);
    if (
      schema.additionalProperties !== undefined &&
      typeof schema.additionalProperties !== "boolean"
    )
      collectRefs(schema.additionalProperties, out);
  };
  const needed = new Set<string>();
  for (const pathItem of Object.values(authority.extension.paths)) {
    for (const [verb, operation] of Object.entries(pathItem)) {
      if (verb === "parameters") continue;
      const success = Object.entries(operation.responses).filter(([status]) =>
        status.startsWith("2"),
      );
      const firstSuccess = success[0]?.[1];
      const envelopeName =
        firstSuccess === undefined
          ? undefined
          : firstSuccess.content["application/json"]?.schema.$ref
              ?.split("/")
              .at(-1);
      const envelope =
        envelopeName === undefined ? undefined : schemas[envelopeName];
      if (typeof envelope !== "boolean" && envelope !== undefined)
        collectRefs(envelope.properties?.result ?? false, needed);
      if (operation.requestBody !== undefined) {
        const content = (
          operation.requestBody as {
            content: Record<string, { schema: Schema }>;
          }
        ).content;
        const body = content["application/json"]?.schema;
        if (body !== undefined) collectRefs(body, needed);
      }
    }
  }
  let pending = [...needed];
  while (pending.length > 0) {
    const name = pending.pop();
    if (name === undefined) continue;
    const schema = schemas[name];
    if (schema === undefined) continue;
    if (typeof schema === "boolean") continue;
    const refs = new Set<string>();
    collectRefs(schema, refs);
    for (const ref of refs)
      if (!needed.has(ref)) {
        needed.add(ref);
        pending.push(ref);
      }
  }
  const internal = new Set([
    "Error",
    "Message",
    "PathSegment",
    "ErrorEnvelope",
  ]);
  const toDelete: string[] = [];
  for (const name of needed) {
    if (name.endsWith("Response") || internal.has(name)) toDelete.push(name);
  }
  for (const name of toDelete) needed.delete(name);
  const declarations: Array<[string, string]> = [];
  for (const name of [...needed].sort()) {
    const schema = schemas[name];
    if (schema === undefined)
      throw new Error(`vendor schema ${name} is missing from the extension`);
    declarations.push([name, typeForSchema(schema)]);
  }
  return declarations;
}

function deepRewriteRefs(value: unknown, prefix: string): unknown {
  if (typeof value === "string") {
    if (value.startsWith("#/components/schemas/"))
      return `#/components/schemas/${prefix}${value.slice("#/components/schemas/".length)}`;
    return value;
  }
  if (Array.isArray(value))
    return value.map((item) => deepRewriteRefs(item, prefix));
  if (value !== null && typeof value === "object") {
    const out: Record<string, unknown> = {};
    for (const [key, item] of Object.entries(value))
      out[key] = deepRewriteRefs(item, prefix);
    return out;
  }
  return value;
}

interface NodeDelegate {
  module: string;
  className: string;
}

function nodeDelegates(mapped: MappedOperation[]): Map<string, NodeDelegate> {
  const delegates = new Map<string, NodeDelegate>();
  for (const operation of mapped) {
    const existing = delegates.get(operation.node);
    if (existing !== undefined) {
      if (
        existing.module !== operation.delegateModule ||
        existing.className !== operation.delegateClass
      )
        throw new Error(`node ${operation.node} maps to conflicting delegates`);
      continue;
    }
    delegates.set(operation.node, {
      module: operation.delegateModule,
      className: operation.delegateClass,
    });
  }
  return delegates;
}

function facadeVariable(node: string): string {
  return node.split(".").map(camelCase).join("");
}

function renderVendorInterfaces(methods: VendorMethod[]): {
  rootMembers: string;
  childBlocks: string;
} {
  interface VendorInterfaceNode {
    children: Map<string, VendorInterfaceNode>;
    method?: VendorMethod;
  }
  const root: VendorInterfaceNode = { children: new Map() };
  for (const method of methods) {
    let current = root;
    for (const segment of method.tree) {
      let child = current.children.get(segment);
      if (child === undefined) {
        child = { children: new Map() };
        current.children.set(segment, child);
      }
      current = child;
    }
    current.method = method;
  }
  const vendorName = (path: string[]): string =>
    path.length === 0
      ? "OpenComputeVendorNode"
      : `OpenComputeVendor${path.map(pascalCase).join("")}Node`;
  const blocks: string[] = [];
  let rootMembers = "";
  const render = (node: VendorInterfaceNode, path: string[]): void => {
    const members: string[] = [];
    for (const [name, child] of [...node.children].sort(([a], [b]) =>
      a.localeCompare(b),
    )) {
      if (child.children.size === 0 && child.method !== undefined) {
        members.push(
          `  readonly ${name}: (${child.method.arguments}) => APIPromise<${child.method.resultType}>;`,
        );
      } else {
        render(child, [...path, name]);
        members.push(`  readonly ${name}: ${vendorName([...path, name])};`);
      }
    }
    if (path.length === 0) rootMembers = members.join("\n");
    else
      blocks.push(
        `export interface ${vendorName(path)} {\n${members.join("\n")}\n}`,
      );
  };
  render(root, []);
  return { rootMembers, childBlocks: blocks.join("\n\n") };
}

function renderGenerated(input: {
  mapped: MappedOperation[];
  typeReExports: Map<string, TypeReExport[]>;
  vendorTypes: Array<[string, string]>;
  vendorMethods: VendorMethod[];
  tree: TreeNode;
  surfaceDigest: string;
}): string {
  const delegates = nodeDelegates(input.mapped);
  const aliases = new Map<string, string>();
  const usedAliases = new Set<string>();
  const importStatements: string[] = [
    `import type { APIPromise } from "cloudflare";`,
    `import type { BaseCloudflare, Cloudflare } from "cloudflare/client";`,
    `import type { VersionCreateParams } from "cloudflare/resources/workers/scripts/versions";`,
    `import type { ScriptUpdateParams } from "cloudflare/resources/workers/scripts/scripts";`,
    `import type { UploadCreateParams } from "cloudflare/resources/workers/assets/upload";`,
    `import { Artifacts } from "./artifacts.ts";`,
  ];
  for (const node of [...delegates.keys()].sort()) {
    const delegate = delegates.get(node);
    if (delegate === undefined) continue;
    let alias = delegate.className;
    if (usedAliases.has(alias)) {
      let suffix = 2;
      while (usedAliases.has(`${delegate.className}${suffix}`)) suffix += 1;
      alias = `${delegate.className}${suffix}`;
    }
    usedAliases.add(alias);
    aliases.set(node, alias);
    const binding =
      alias === delegate.className
        ? delegate.className
        : `${delegate.className} as ${alias}`;
    importStatements.push(
      `import { ${binding} } from "cloudflare/${delegate.module}";`,
    );
  }
  const instantiationLines = [...delegates.keys()].sort().map((node) => {
    const delegate = delegates.get(node);
    const alias = aliases.get(node);
    if (delegate === undefined || alias === undefined)
      throw new Error(`missing delegate for ${node}`);
    return `  const ${facadeVariable(node)} = new ${alias}(transport);`;
  });
  const vendorTypeDeclarations = input.vendorTypes
    .map(([name, type]) => `export type ${name} = ${type};`)
    .join("\n\n");
  const vendorTree = renderVendorInterfaces(input.vendorMethods);
  const surfaceMembers = [...input.tree.children.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name]) => `  readonly ${name}: ${nodeInterfaceName(name)};`)
    .join("\n");
  const reExportLines = [...input.typeReExports.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(
      ([module, entries]) =>
        `export type { ${entries
          .map((entry) =>
            entry.as === undefined
              ? entry.name
              : `${entry.name} as ${entry.as}`,
          )
          .join(", ")} } from "cloudflare/${module}";`,
    );
  return `// Generated by scripts/generate.ts from the pinned open-compute OpenAPI authority. Do not edit.
${importStatements.join("\n")}

/** Official transport request options reused by vendor operations. */
export type OpenComputeRequestOptions = Cloudflare.RequestOptions;

/** Native Dynamic Worker Loader binding accepted by open-compute and Wrangler. */
export type OpenComputeWorkerLoaderBinding = {
  readonly type: "worker_loader";
  readonly name: string;
};

type OfficialWorkerVersionBinding = NonNullable<
  VersionCreateParams["metadata"]["bindings"]
>[number];

type OfficialWorkerScriptBinding = NonNullable<
  ScriptUpdateParams["metadata"]["bindings"]
>[number];

/** Official script upload parameters plus the runtime-supported Worker Loader binding. */
export type OpenComputeWorkerScriptUpdateParams = Omit<
  ScriptUpdateParams,
  "metadata"
> & {
  readonly metadata: Omit<ScriptUpdateParams["metadata"], "bindings"> & {
    readonly bindings?: readonly (
      | OfficialWorkerScriptBinding
      | OpenComputeWorkerLoaderBinding
    )[];
  };
};

// The official update delegate sets application/javascript even for multipart files.
// Null removes that default so the request encoder supplies the form boundary.
function scriptUploadOptions(options?: OpenComputeRequestOptions): OpenComputeRequestOptions {
  const original = options?.headers;
  const entries = original instanceof Headers
    ? [...original.entries()]
    : Array.isArray(original)
      ? [...original]
      : Object.entries(original ?? {});
  return { ...options, headers: [...entries, ["Content-Type", null]] };
}

/** Official Version upload parameters plus the runtime-supported Worker Loader binding. */
export type OpenComputeWorkerVersionCreateParams = Omit<
  VersionCreateParams,
  "metadata"
> & {
  readonly metadata: Omit<VersionCreateParams["metadata"], "bindings"> & {
    readonly bindings?: readonly (
      | OfficialWorkerVersionBinding
      | OpenComputeWorkerLoaderBinding
    )[];
  };
};

/** Official Static Assets upload parameters with browser-native File parts. */
export type OpenComputeAssetsUploadCreateParams = Omit<
  UploadCreateParams,
  "body"
> & {
  readonly body: Record<string, string | File>;
};

/** The Cloudflare v4 success envelope returned by every vendor operation. */
export interface V4Envelope<T> {
  readonly success: true;
  readonly result: T;
  readonly errors: readonly unknown[];
  readonly messages: readonly unknown[];
}

function segment(value: string): string {
  if (value.length === 0 || value === "." || value === "..")
    throw new Error("invalid vendor path segment");
  return encodeURIComponent(value);
}

${vendorTypeDeclarations}

function buildOpenCompute(transport: BaseCloudflare): OpenComputeVendorNode {
  return {
${renderVendorTree(input.vendorMethods, "    ")}
  };
}

${renderNodeInterfaces(input.tree, [], delegates, aliases)}

${vendorTree.childBlocks}

export interface OpenComputeVendorNode {
${vendorTree.rootMembers}
}

/**
 * The closed capability-scoped surface: every selected official operation,
 * the vendor namespace, and nothing else.
 */
export interface OpenComputeSurface {
${surfaceMembers}
  readonly artifacts: Artifacts;
  readonly openCompute: OpenComputeVendorNode;
}

/**
 * Build the closed capability-scoped surface over one hidden official
 * transport. The returned object exposes exactly the selected operations and
 * nothing else; every method delegates to the official SDK implementation.
 */
export function buildFacade(transport: BaseCloudflare): OpenComputeSurface {
${instantiationLines.join("\n")}
  return {
${renderRuntimeTree(input.tree, "    ")}
    artifacts: new Artifacts(transport),
    openCompute: buildOpenCompute(transport),
  };
}

${reExportLines.join("\n")}
`;
}

function canonicalDigest(value: unknown): string {
  return sha256(JSON.stringify(value));
}

function renderSurfaceReport(input: {
  authority: Authority;
  packageVersion: string;
  lock: LockAuthority;
  mapped: MappedOperation[];
  excluded: ManifestEntry[];
  vendorMethods: VendorMethod[];
  surfaceDigest: string;
}): string {
  const report = {
    schemaVersion: 1,
    package: "@open-compute/sdk",
    packageVersion: input.packageVersion,
    surfaceDigest: input.surfaceDigest,
    authority: {
      openapiRevision: input.lock.revision,
      openapiSha256: input.lock.sha256,
      cloudflareSdkVersion: input.lock.cloudflareSdk.version,
      cloudflareSdkNpmIntegrity: input.lock.cloudflareSdk.npmIntegrity,
      subsetSha256: input.lock.subsetSha256,
      extensionSha256: input.lock.extensionSha256,
      observedStandardSha256: input.lock.observedStandardSha256,
    },
    operations: input.mapped
      .map((operation) => ({
        operation: operation.operation,
        operationId: operation.operationId,
        method: operation.method,
        path: operation.path,
        status: operation.status,
        deviations: operation.deviations,
        operationSha256: operation.operationSha256,
        node: operation.node,
        officialMethod: operation.officialMethod,
        delegateModule: operation.delegateModule,
        delegateClass: operation.delegateClass,
        source: "official",
      }))
      .sort((left, right) => left.operation.localeCompare(right.operation)),
    excludedOperations: input.excluded,
    openComputeOperations: input.vendorMethods.map((method) => ({
      source: "open_compute_extension",
      node: `openCompute.${method.tree.join(".")}`,
      method: method.verb.toUpperCase(),
      path: method.openapiPath,
    })),
    observedStandardOperations: Object.entries(
      input.authority.observed.paths,
    ).flatMap(([path, methods]) =>
      Object.entries(methods).map(([method, operation]) => ({
        source: "observed_standard",
        node: operation["x-open-compute-sdk-method"],
        operationId: operation.operationId,
        operationSha256: sha256(JSON.stringify(operation)),
        method: method.toUpperCase(),
        path,
        delegateModule: "src/artifacts.ts",
        delegateClass: "Artifacts",
      })),
    ),
  };
  return `${JSON.stringify(report, null, 2)}\n`;
}

function renderCombinedOpenAPI(input: {
  authority: Authority;
  packageVersion: string;
  mapped: MappedOperation[];
  excluded: ManifestEntry[];
  surfaceDigest: string;
}): string {
  const { authority } = input;
  const statusByOperation = new Map(
    authority.subset["x-open-compute-operation-inventory"].map((entry) => [
      entry.id,
      entry,
    ]),
  );
  const excludedSet = new Set(input.excluded.map((entry) => entry.operation));
  const nodeByOperation = new Map(
    input.mapped.map((operation) => [operation.operation, operation]),
  );
  const paths: Record<string, Record<string, unknown>> = {};
  const operationIds = new Set<string>();
  for (const [path, pathItem] of Object.entries(authority.subset.paths)) {
    for (const [verb, operation] of Object.entries(pathItem)) {
      if (verb === "parameters") continue;
      const id = `${verb.toUpperCase()} ${path}`;
      const entry = statusByOperation.get(id);
      if (entry === undefined)
        throw new Error(`subset operation ${id} is missing from the inventory`);
      if (
        (entry.status !== "supported" &&
          entry.status !== "supported_with_deviation") ||
        excludedSet.has(id)
      )
        continue;
      const mapped = nodeByOperation.get(id);
      if (mapped === undefined)
        throw new Error(`subset operation ${id} was not mapped`);
      if (operationIds.has(entry.operationId))
        throw new Error(
          `duplicate operationId in combined surface: ${entry.operationId}`,
        );
      operationIds.add(entry.operationId);
      const item = (paths[path] ??= {});
      if (pathItem.parameters !== undefined && item.parameters === undefined)
        item.parameters = (pathItem as { parameters: unknown }).parameters;
      item[verb] = {
        ...(operation as Record<string, unknown>),
        "x-open-compute-status": entry.status,
        ...(entry.status === "supported_with_deviation"
          ? { "x-open-compute-deviations": mapped.deviations }
          : {}),
        "x-open-compute-sdk-node": mapped.node,
      };
    }
  }
  const vendorPaths = deepRewriteRefs(
    authority.extension.paths,
    "OpenCompute",
  ) as Record<string, Record<string, unknown>>;
  const observedPaths = deepRewriteRefs(
    authority.observed.paths,
    "Artifacts",
  ) as Record<
    string,
    Record<string, { operationId: string; "x-open-compute-sdk-method": string }>
  >;
  for (const [path, pathItem] of Object.entries(observedPaths)) {
    if (paths[path] !== undefined)
      throw new Error(
        `observed-standard path collides with the selected surface: ${path}`,
      );
    const copied: Record<string, unknown> = {};
    for (const [verb, operation] of Object.entries(pathItem)) {
      if (operationIds.has(operation.operationId))
        throw new Error(`duplicate operationId: ${operation.operationId}`);
      operationIds.add(operation.operationId);
      copied[verb] = {
        ...operation,
        "x-open-compute-source": "observed_standard",
        "x-open-compute-sdk-node": operation["x-open-compute-sdk-method"],
      };
    }
    paths[path] = copied;
  }
  for (const [path, pathItem] of Object.entries(vendorPaths)) {
    if (paths[path] !== undefined)
      throw new Error(
        `vendor path collides with the selected surface: ${path}`,
      );
    for (const [verb, operation] of Object.entries(pathItem)) {
      if (verb === "parameters") continue;
      const vendor = operation as VendorOperation;
      if (operationIds.has(vendor.operationId))
        throw new Error(`duplicate operationId: ${vendor.operationId}`);
      operationIds.add(vendor.operationId);
      pathItem[verb] = {
        ...vendor,
        "x-open-compute-sdk-node": `openCompute.${vendor["x-open-compute-sdk-method"]}`,
      };
    }
    paths[path] = pathItem;
  }
  const schemas: Record<string, unknown> = {
    ...authority.subset.components.schemas,
  };
  for (const [name, schema] of Object.entries(
    authority.extension.components.schemas,
  )) {
    const prefixed = `OpenCompute${name}`;
    if (schemas[prefixed] !== undefined)
      throw new Error(`component name collision: ${prefixed}`);
    schemas[prefixed] = deepRewriteRefs(schema, "OpenCompute");
  }
  for (const [name, schema] of Object.entries(
    authority.observed.components.schemas,
  )) {
    const prefixed = `Artifacts${name}`;
    if (schemas[prefixed] !== undefined)
      throw new Error(
        `observed-standard component name collision: ${prefixed}`,
      );
    schemas[prefixed] = deepRewriteRefs(schema, "Artifacts");
  }
  const document = {
    openapi: "3.0.3",
    info: {
      title: "open-compute SDK surface",
      version: input.packageVersion,
      description:
        "Combined Cloudflare-compatible and vendor surface exposed by @open-compute/sdk. Generated from the pinned authority; do not hand-edit.",
    },
    servers: [{ url: "/client/v4" }],
    "x-open-compute-upstream": authority.subset["x-open-compute-upstream"],
    "x-open-compute-sdk": {
      package: "@open-compute/sdk",
      packageVersion: input.packageVersion,
      surfaceDigest: input.surfaceDigest,
      openapiRevision: authority.lock.revision,
      cloudflareSdk: authority.lock.cloudflareSdk.version,
    },
    paths,
    components: {
      parameters: deepRewriteRefs(
        authority.observed.components.parameters,
        "Artifacts",
      ),
      schemas,
      requestBodies: deepRewriteRefs(
        authority.observed.components.requestBodies,
        "Artifacts",
      ),
      responses: deepRewriteRefs(
        authority.observed.components.responses,
        "Artifacts",
      ),
    },
  };
  return `${JSON.stringify(document, null, 2)}\n`;
}

function formatGenerated(source: string): string {
  const prettier = spawnSync(
    resolve(REPO_ROOT, "node_modules/.bin/prettier"),
    ["--stdin-filepath", "generated.ts"],
    { input: source, encoding: "utf8" },
  );
  if (prettier.error !== undefined && prettier.error !== null)
    throw prettier.error;
  if (prettier.status !== 0)
    throw new Error(
      `failed to format generated SDK source: ${prettier.stderr}`,
    );
  return prettier.stdout;
}

async function main(): Promise<void> {
  const authority = loadAuthority();
  const scan = await scanOfficialPackage(
    resolve(PACKAGE_ROOT, "node_modules/cloudflare"),
  );
  const routeIndex = buildRouteIndex(scan);
  const { mapped, excluded } = mapOperations(authority, scan, routeIndex);
  const typeReExports = planTypeReExports(mapped, scan);
  const tree = buildTree(mapped);
  const vendorMethods = collectVendorMethods(authority);
  const vendorTypes = vendorReachableTypes(authority);
  const packageJson = readJson(resolve(PACKAGE_ROOT, "package.json")) as {
    version: string;
  };
  const surfaceDigest = canonicalDigest({
    operations: [...mapped].sort((left, right) =>
      left.operation.localeCompare(right.operation),
    ),
    excludedOperations: excluded,
    openComputeOperations: vendorMethods.map((method) => ({
      node: method.tree.join("."),
      method: method.verb.toUpperCase(),
      path: method.template,
      arguments: method.arguments,
      resultType: method.resultType,
    })),
    observedStandardOperations: authority.observed.paths,
  });
  const rawGenerated = renderGenerated({
    mapped,
    typeReExports,
    vendorTypes,
    vendorMethods,
    tree,
    surfaceDigest,
  });
  if (process.env.OPEN_COMPUTE_SDK_RAW_GENERATED !== undefined)
    writeFileSync(process.env.OPEN_COMPUTE_SDK_RAW_GENERATED, rawGenerated);
  const generated = formatGenerated(rawGenerated);
  const surface = renderSurfaceReport({
    authority,
    packageVersion: packageJson.version,
    lock: authority.lock,
    mapped,
    excluded,
    vendorMethods,
    surfaceDigest,
  });
  const combined = renderCombinedOpenAPI({
    authority,
    packageVersion: packageJson.version,
    mapped,
    excluded,
    surfaceDigest,
  });
  const outputs: Array<{ path: string; content: string }> = [
    { path: resolve(PACKAGE_ROOT, "src/generated.ts"), content: generated },
    { path: resolve(PACKAGE_ROOT, "surface.json"), content: surface },
    {
      path: resolve(REPO_ROOT, "openapi/open-compute-sdk.json"),
      content: combined,
    },
  ];
  if (CHECK) {
    const drifted: string[] = [];
    for (const output of outputs) {
      let current: string | undefined;
      try {
        current = readFileSync(output.path, "utf8");
      } catch {
        current = undefined;
      }
      if (current !== output.content) drifted.push(output.path);
    }
    if (drifted.length > 0) {
      for (const path of drifted)
        console.error(`generated SDK output drifted: ${path}`);
      console.error("run `bun run generate` in packages/sdk to regenerate");
      process.exitCode = 1;
      return;
    }
    return;
  }
  for (const output of outputs) writeFileSync(output.path, output.content);
  console.log(
    `generated ${mapped.length} standard operations, ${vendorMethods.length} vendor operations, ${excluded.length} exclusions`,
  );
}

await main();
