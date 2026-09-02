import { constants } from "node:fs";
import { lstat, open, opendir, realpath } from "node:fs/promises";
import { dirname, extname, isAbsolute, relative, resolve, sep } from "node:path";
import type { CompiledModule, CompiledModuleType, CompiledWorker } from "../build-worker.ts";
import type { JsonValue, RuntimeFeatures, WorkerBinding, WorkerProject, WorkerService } from "../project.ts";
import { parseRuntimeFeatures, record } from "../project.ts";
import { loadFormalRuntimeLock, type FormalRuntimeLock } from "../runtime-lock.ts";
import type { AssetsProject } from "../assets/types.ts";

const CURRENT_DEFAULT_FLAGS = new Set(["nodejs_compat", "nodejs_compat_v2", "rpc", "enable_ctx_exports"]);

const MAX_CONFIG_BYTES = 256 * 1024;
const MAX_MODULES = 128;
const MAX_MODULE_BYTES = 4 * 1024 * 1024;
const MAX_TOTAL_MODULE_BYTES = 16 * 1024 * 1024;
const GENERATED_CONFIG_KEYS = [
  "configPath", "userConfigPath", "topLevelName", "definedEnvironments", "compatibility_date",
  "compatibility_flags", "jsx_factory", "jsx_fragment", "rules", "name", "main", "triggers",
  "assets", "vars", "define", "durable_objects", "workflows", "migrations", "exports",
  "kv_namespaces", "cloudchamber", "send_email", "queues", "connect", "r2_buckets", "ai",
  "d1_databases", "vectorize", "ai_search_namespaces", "ai_search", "agent_memory", "hyperdrive", "browser",
  "services", "analytics_engine_datasets", "dispatch_namespaces", "mtls_certificates", "images",
  "pipelines", "secrets_store_secrets", "artifacts", "unsafe_hello_world", "flagship",
  "worker_loaders", "ratelimits", "vpc_services", "vpc_networks", "version_metadata", "logfwdr", "unsafe",
  "cache", "python_modules", "dev", "no_bundle",
] as const;
const UNSUPPORTED_GENERATED_BINDING_KEYS = [
  "define", "dispatch_namespaces", "hyperdrive", "browser",
  "mtls_certificates", "unsafe", "cloudchamber", "send_email", "connect",
  "analytics_engine_datasets", "agent_memory", "pipelines",
  "secrets_store_secrets", "artifacts", "unsafe_hello_world", "flagship", "worker_loaders",
  "ratelimits", "vpc_services", "vpc_networks", "logfwdr",
] as const;

interface ModuleRule {
  readonly type: CompiledModuleType;
  readonly globs: readonly RegExp[];
  readonly fallthrough: boolean;
}

interface GeneratedBinding {
  readonly type: WorkerBinding["type"];
  readonly className?: string;
}

/** Immutable framework build imported from a generated Wrangler deployment description. */
export interface FrameworkOutput {
  readonly worker: CompiledWorker;
  readonly assets?: AssetsProject;
  readonly services: Record<string, WorkerService>;
  readonly runtimeFeatures: RuntimeFeatures;
}

function assertImportedCompatibility(
  date: unknown,
  flags: unknown,
  lock: FormalRuntimeLock,
): void {
  if (date !== lock.effectiveCompatibilityDate) {
    throw new Error("generated framework compatibility date does not match the pinned runtime lock");
  }
  const generated = flags === undefined ? [] : flags;
  if (!Array.isArray(generated) || !generated.every((item): item is string => typeof item === "string")) {
    throw new Error("generated framework compatibility flags are invalid");
  }
  const seen = new Set<string>();
  for (const flag of generated) {
    if (seen.has(flag)) throw new Error("generated framework compatibility flags are duplicated");
    seen.add(flag);
    if (flag.startsWith("no_") || flag === "experimental" || lock.systemCompatibilityFlags.includes(flag)) {
      throw new Error("generated framework compatibility flag is not part of the pinned baseline");
    }
    if (CURRENT_DEFAULT_FLAGS.has(flag)) continue;
    if (!lock.requiredCompatibilityFlags.includes(flag)) {
      throw new Error("generated framework compatibility flag is not part of the pinned baseline");
    }
  }
}

function within(root: string, value: string): boolean {
  const child = relative(root, value);
  return child !== ".." && !child.startsWith(`..${sep}`) && !isAbsolute(child);
}

async function boundedJson(filename: string, label: string): Promise<Record<string, unknown>> {
  const file = await open(filename, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > MAX_CONFIG_BYTES) throw new Error(`${label} must be a bounded regular JSON file`);
    const bytes = Buffer.alloc(info.size);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) break;
      offset += bytesRead;
    }
    if (offset !== bytes.length) throw new Error(`${label} is truncated`);
    let value: unknown;
    try { value = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)); }
    catch { throw new Error(`${label} must be valid UTF-8 JSON`); }
    if (!record(value)) throw new Error(`${label} must contain one JSON object`);
    return value;
  } finally { await file.close(); }
}

function onlyKeys(value: Record<string, unknown>, keys: readonly string[], label: string): void {
  if (Object.keys(value).some(key => !keys.includes(key))) throw new Error(`${label} contains an unsupported field`);
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || !value || /[\0\r\n]/.test(value)) throw new Error(`${label} is invalid`);
  return value;
}

function bindingName(value: unknown, label: string): string {
  const name = string(value, label);
  if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name)) throw new Error(`${label} is invalid`);
  return name;
}

function className(value: unknown, label: string): string {
  const name = string(value, label);
  if (!/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(name)) throw new Error(`${label} is invalid`);
  return name;
}

function generatedArray(value: unknown, label: string, limit = 128): Record<string, unknown>[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > limit || !value.every(record)) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function emptyGeneratedDeclaration(value: unknown): boolean {
  if (value === undefined) return true;
  if (Array.isArray(value)) return value.length === 0;
  if (record(value)) return Object.values(value).every(emptyGeneratedDeclaration);
  return false;
}

function canonicalJson(value: JsonValue): JsonValue {
  if (Array.isArray(value)) return value.map(canonicalJson);
  if (!record(value)) return value;
  return Object.fromEntries(Object.keys(value).sort().map(key => [key, canonicalJson(value[key] as JsonValue)]));
}

function generatedBindings(config: Record<string, unknown>, project: WorkerProject): void {
  const generatedVars = config.vars ?? {};
  if (!record(generatedVars)
      || JSON.stringify(canonicalJson(generatedVars as Record<string, JsonValue>))
        !== JSON.stringify(canonicalJson(project.vars))) {
    throw new Error("generated framework vars conflict with the project");
  }
  for (const key of UNSUPPORTED_GENERATED_BINDING_KEYS) {
    if (!emptyGeneratedDeclaration(config[key])) {
      throw new Error(`generated framework config declares unsupported ${key}`);
    }
  }
  const declarations = new Map<string, GeneratedBinding>();
  const add = (name: string, declaration: GeneratedBinding): void => {
    if (declarations.has(name)) throw new Error("generated framework binding names conflict");
    declarations.set(name, declaration);
  };
  for (const item of generatedArray(config.kv_namespaces, "generated framework KV bindings")) {
    onlyKeys(item, ["binding", "id", "preview_id", "remote"], "generated framework KV binding");
    add(bindingName(item.binding, "generated framework KV binding name"), { type: "kv_namespace" });
  }
  for (const item of generatedArray(config.r2_buckets, "generated framework R2 bindings")) {
    onlyKeys(item, ["binding", "bucket_name", "preview_bucket_name"], "generated framework R2 binding");
    add(bindingName(item.binding, "generated framework R2 binding name"), { type: "r2_bucket" });
  }
  for (const item of generatedArray(config.d1_databases, "generated framework D1 bindings")) {
    onlyKeys(item, ["binding", "database_id", "database_name", "preview_database_id"], "generated framework D1 binding");
    add(bindingName(item.binding, "generated framework D1 binding name"), { type: "d1_database" });
  }
  if (config.durable_objects !== undefined) {
    if (!record(config.durable_objects)) throw new Error("generated framework Durable Object bindings are invalid");
    onlyKeys(config.durable_objects, ["bindings"], "generated framework Durable Object config");
    for (const item of generatedArray(config.durable_objects.bindings, "generated framework Durable Object bindings")) {
      onlyKeys(item, ["name", "class_name", "script_name", "environment"], "generated framework Durable Object binding");
      add(bindingName(item.name, "generated framework Durable Object binding name"), {
        type: "do_namespace",
        className: className(item.class_name, "generated framework Durable Object class"),
      });
    }
  }
  if (config.queues !== undefined) {
    if (!record(config.queues)) throw new Error("generated framework Queue bindings are invalid");
    onlyKeys(config.queues, ["producers", "consumers"], "generated framework Queue config");
    if (generatedArray(config.queues.consumers, "generated framework Queue consumers").length) {
      throw new Error("generated framework Queue consumers are unsupported");
    }
    for (const item of generatedArray(config.queues.producers, "generated framework Queue producers")) {
      onlyKeys(item, ["binding", "queue"], "generated framework Queue producer");
      add(bindingName(item.binding, "generated framework Queue producer binding"), { type: "queue_producer" });
    }
  }
  for (const item of generatedArray(config.workflows, "generated framework Workflow bindings")) {
    onlyKeys(item, ["binding", "name", "class_name", "script_name"], "generated framework Workflow binding");
    add(bindingName(item.binding, "generated framework Workflow binding name"), {
      type: "workflow",
      className: className(item.class_name, "generated framework Workflow class"),
    });
  }
  for (const item of generatedArray(config.vectorize, "generated framework Vectorize bindings")) {
    onlyKeys(item, ["binding", "index_name", "remote"], "generated framework Vectorize binding");
    string(item.index_name, "generated framework Vectorize index name");
    if (item.remote !== undefined && typeof item.remote !== "boolean") throw new Error("generated framework Vectorize remote selector is invalid");
    add(bindingName(item.binding, "generated framework Vectorize binding name"), { type: "vectorize_index" });
  }
  for (const item of generatedArray(config.ai_search_namespaces, "generated framework AI Search namespace bindings")) {
    onlyKeys(item, ["binding", "namespace", "remote"], "generated framework AI Search namespace binding");
    string(item.namespace, "generated framework AI Search namespace");
    if (item.remote !== undefined && typeof item.remote !== "boolean") throw new Error("generated framework AI Search namespace remote selector is invalid");
    add(bindingName(item.binding, "generated framework AI Search namespace binding name"), { type: "ai_search_namespace" });
  }
  for (const item of generatedArray(config.ai_search, "generated framework AI Search instance bindings")) {
    onlyKeys(item, ["binding", "instance_name", "remote"], "generated framework AI Search instance binding");
    string(item.instance_name, "generated framework AI Search instance name");
    if (item.remote !== undefined && typeof item.remote !== "boolean") throw new Error("generated framework AI Search instance remote selector is invalid");
    add(bindingName(item.binding, "generated framework AI Search instance binding name"), { type: "ai_search_instance" });
  }
  if (declarations.size !== Object.keys(project.bindings).length) {
    throw new Error("generated framework bindings differ from the project");
  }
  for (const [name, local] of Object.entries(project.bindings)) {
    const generated = declarations.get(name);
    if (generated === undefined || generated.type !== local.type
        || generated.className !== local.className) {
      throw new Error("generated framework bindings differ from the project");
    }
  }
}

function validateGeneratedConfig(config: Record<string, unknown>): void {
  onlyKeys(config, GENERATED_CONFIG_KEYS, "generated framework config");
  if (config.no_bundle !== true) throw new Error("generated framework config must preserve the prebuilt module graph");
  if (!emptyGeneratedDeclaration(config.triggers)) throw new Error("generated framework triggers are unsupported");
  if (!emptyGeneratedDeclaration(config.migrations)) throw new Error("generated framework migrations are unsupported");
  if (config.definedEnvironments !== undefined
      && (!Array.isArray(config.definedEnvironments) || config.definedEnvironments.length !== 0)) {
    throw new Error("generated framework environments are unsupported");
  }
}

function serviceDeclarations(value: unknown): Record<string, WorkerService> {
  const services: Record<string, WorkerService> = {};
  if (value === undefined) return services;
  if (!Array.isArray(value) || value.length > 64) {
    throw new Error("generated framework services are invalid");
  }
  for (const item of value) {
    if (!record(item)) throw new Error("generated framework service is invalid");
    onlyKeys(item, ["binding", "service", "entrypoint"], "generated framework service");
    const binding = string(item.binding, "generated framework service binding");
    const service = string(item.service, "generated framework service target");
    const entrypoint = item.entrypoint === undefined
      ? undefined : string(item.entrypoint, "generated framework service entrypoint");
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(binding)
        || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(service)
        || (entrypoint !== undefined && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypoint))
        || Object.hasOwn(services, binding)) {
      throw new Error("generated framework service is invalid");
    }
    Object.defineProperty(services, binding, {
      value: { service, ...(entrypoint === undefined ? {} : { entrypoint }) },
      enumerable: true,
    });
  }
  return services;
}

function reconciledServices(value: unknown, local: Record<string, WorkerService>): Record<string, WorkerService> {
  const generated = serviceDeclarations(value);
  if (Object.keys(generated).length !== Object.keys(local).length) {
    throw new Error("generated framework services differ from the project");
  }
  for (const [binding, declaration] of Object.entries(local)) {
    const expected = generated[binding];
    if (expected === undefined || expected.entrypoint !== declaration.entrypoint) {
      throw new Error("generated framework services differ from the project");
    }
  }
  return { ...local };
}

function outputPath(root: string, base: string, value: unknown, label: string): Promise<string> {
  const path = resolve(base, string(value, label));
  if (!within(root, path)) throw new Error(`${label} must stay inside the project`);
  return lstat(path).then(info => {
    if (info.isSymbolicLink()) throw new Error(`${label} must not be a symbolic link`);
    return realpath(path);
  }).then(actual => {
    if (!within(root, actual)) throw new Error(`${label} escapes the project`);
    return actual;
  });
}

function glob(pattern: string): RegExp {
  if (!pattern || pattern.startsWith("/") || pattern.includes("\\") || pattern.includes("\0")) {
    throw new Error("framework module rule glob is invalid");
  }
  let source = "^";
  for (let index = 0; index < pattern.length; index += 1) {
    const character = pattern[index]!;
    if (character === "*") {
      if (pattern[index + 1] === "*" && pattern[index + 2] === "/") {
        source += "(?:.*/)?";
        index += 2;
      } else if (pattern[index + 1] === "*") { source += ".*"; index += 1; }
      else source += "[^/]*";
    } else if (character === "?") source += "[^/]";
    else source += character.replace(/[\\^$+?.()|[\]{}]/g, "\\$&");
  }
  return new RegExp(`${source}$`);
}

function moduleType(value: unknown): CompiledModuleType {
  switch (value) {
    case "ESModule": return "esModule";
    case "CommonJS": return "commonJsModule";
    case "Text": return "text";
    case "Data": return "data";
    case "CompiledWasm": return "wasm";
    default: throw new Error("framework module rule type is unsupported");
  }
}

function moduleRules(value: unknown): ModuleRule[] {
  if (value === undefined) return [];
  if (!Array.isArray(value) || value.length > 32) throw new Error("framework module rules are invalid");
  return value.map(item => {
    if (!record(item)) throw new Error("framework module rule is invalid");
    onlyKeys(item, ["type", "globs", "fallthrough"], "framework module rule");
    if (!Array.isArray(item.globs) || !item.globs.length || item.globs.length > 64
        || !item.globs.every((entry): entry is string => typeof entry === "string")) {
      throw new Error("framework module rule globs are invalid");
    }
    if (item.fallthrough !== undefined && typeof item.fallthrough !== "boolean") {
      throw new Error("framework module rule fallthrough is invalid");
    }
    return { type: moduleType(item.type), globs: item.globs.map(glob), fallthrough: item.fallthrough === true };
  });
}

function classify(name: string, rules: readonly ModuleRule[]): CompiledModuleType | undefined {
  for (const rule of rules) {
    if (rule.globs.some(pattern => pattern.test(name))) return rule.type;
  }
  switch (extname(name).toLowerCase()) {
    case ".mjs": return "esModule";
    case ".js": case ".cjs": return "commonJsModule";
    case ".wasm": return "wasm";
    case ".txt": case ".html": case ".sql": return "text";
    case ".bin": return "data";
    default: return undefined;
  }
}

async function readModule(filename: string, name: string, type: CompiledModuleType): Promise<CompiledModule> {
  const file = await open(filename, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > MAX_MODULE_BYTES) throw new Error("framework module exceeds 4 MiB");
    const bytes = Buffer.alloc(info.size);
    let offset = 0;
    while (offset < bytes.length) {
      const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
      if (!bytesRead) break;
      offset += bytesRead;
    }
    const after = await file.stat();
    if (offset !== bytes.length || after.size !== info.size || after.mtimeMs !== info.mtimeMs) {
      throw new Error("framework module changed during import");
    }
    if ((type === "esModule" || type === "commonJsModule" || type === "text" || type === "json")) {
      try { new TextDecoder("utf-8", { fatal: true }).decode(bytes); }
      catch { throw new Error("framework text module is not UTF-8"); }
    }
    if (type === "json") {
      try { JSON.parse(new TextDecoder().decode(bytes)); }
      catch { throw new Error("framework JSON module is invalid"); }
    }
    return { name, type, bytes };
  } finally { await file.close(); }
}

async function importModules(
  root: string,
  main: string,
  assetsDirectory: string | undefined,
  rules: readonly ModuleRule[],
): Promise<CompiledWorker> {
  const modules: CompiledModule[] = [];
  let total = 0;
  const walk = async (directory: string): Promise<void> => {
    const listing = await opendir(directory);
    try {
      for await (const item of listing) {
        const filename = resolve(directory, item.name);
        if (assetsDirectory !== undefined && (filename === assetsDirectory || within(assetsDirectory, filename))) continue;
        const logical = relative(root, filename).split(sep).join("/");
        if (item.isSymbolicLink()) throw new Error("framework output contains a symbolic link");
        if (item.isDirectory()) { await walk(filename); continue; }
        if (!item.isFile()) throw new Error("framework output contains a special file");
        const type = classify(logical, rules);
        if (type === undefined) continue;
        if (modules.length >= MAX_MODULES) throw new Error("framework output exceeds 128 modules");
        const module = await readModule(filename, logical, type);
        total += module.bytes.byteLength;
        if (total > MAX_TOTAL_MODULE_BYTES) throw new Error("framework output exceeds 16 MiB of modules");
        modules.push(module);
      }
    } finally {
      try { await listing.close(); } catch { /* Exhausted async iterators close themselves. */ }
    }
  };
  await walk(root);
  modules.sort((left, right) => Buffer.compare(Buffer.from(left.name), Buffer.from(right.name)));
  const mainModule = relative(root, main).split(sep).join("/");
  if (!modules.some(module => module.name === mainModule && module.type === "esModule")) {
    throw new Error("framework main module is missing or is not an ES module");
  }
  return { mainModule, modules };
}

function routing(value: unknown, directory: string): AssetsProject {
  if (!record(value)) throw new Error("generated framework assets config is invalid");
  onlyKeys(value, ["directory", "binding", "run_worker_first", "html_handling", "not_found_handling"], "generated framework assets config");
  const runWorkerFirst = value.run_worker_first ?? false;
  if (typeof runWorkerFirst !== "boolean" && (!Array.isArray(runWorkerFirst) || !runWorkerFirst.length
      || runWorkerFirst.length > 100 || !runWorkerFirst.every((item): item is string =>
        typeof item === "string" && item.length > 0 && item.length <= 2048
        && (item.startsWith("/") || item.startsWith("!/"))))) {
    throw new Error("generated framework run_worker_first is invalid");
  }
  const htmlHandling = value.html_handling ?? "auto-trailing-slash";
  if (!(["auto-trailing-slash", "force-trailing-slash", "drop-trailing-slash", "none"] as const).includes(htmlHandling as never)) {
    throw new Error("generated framework html_handling is invalid");
  }
  const notFoundHandling = value.not_found_handling ?? "none";
  if (!(["none", "404-page", "single-page-application"] as const).includes(notFoundHandling as never)) {
    throw new Error("generated framework not_found_handling is invalid");
  }
  const binding = value.binding;
  if (binding !== undefined && (typeof binding !== "string" || !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(binding))) {
    throw new Error("generated framework assets binding is invalid");
  }
  return {
    directory,
    ...(binding === undefined ? {} : { binding }),
    runWorkerFirst,
    htmlHandling: htmlHandling as AssetsProject["htmlHandling"],
    notFoundHandling: notFoundHandling as AssetsProject["notFoundHandling"],
    publishSourceMaps: false,
  };
}

/** Import one already-built Vite/vinext Worker without rebundling its module graph. */
export async function importFrameworkOutput(project: WorkerProject): Promise<FrameworkOutput> {
  if (project.frameworkOutput === undefined) throw new Error("project has no frameworkOutput");
  const projectRoot = await realpath(project.project);
  const redirectPath = await outputPath(projectRoot, projectRoot, project.frameworkOutput, "frameworkOutput");
  const redirect = await boundedJson(redirectPath, "framework deploy config");
  onlyKeys(redirect, ["configPath", "auxiliaryWorkers"], "framework deploy config");
  if (redirect.auxiliaryWorkers !== undefined
      && (!Array.isArray(redirect.auxiliaryWorkers) || redirect.auxiliaryWorkers.length !== 0)) {
    throw new Error("framework auxiliary Workers are unsupported");
  }
  const configPath = await outputPath(projectRoot, dirname(redirectPath), redirect.configPath, "generated framework configPath");
  const config = await boundedJson(configPath, "generated framework config");
  validateGeneratedConfig(config);
  const generatedName = config.name;
  if (generatedName !== undefined
      && (typeof generatedName !== "string"
        || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(generatedName))) {
    throw new Error("generated framework Worker name is invalid");
  }
  assertImportedCompatibility(config.compatibility_date, config.compatibility_flags, await loadFormalRuntimeLock());
  generatedBindings(config, project);
  const services = reconciledServices(config.services, project.services);
  const hasGeneratedRuntimeFeatures = ["cache", "exports", "images", "ai", "version_metadata"]
    .some(key => config[key] !== undefined);
  const generatedRuntimeFeatures = hasGeneratedRuntimeFeatures
    ? parseRuntimeFeatures(config, new Set([
      ...Object.keys(project.vars), ...project.secrets, ...Object.keys(project.bindings),
      ...Object.keys(services), ...(project.assets?.binding === undefined ? [] : [project.assets.binding]),
    ]))
    : undefined;
  if (generatedRuntimeFeatures !== undefined) {
    const projectExplicit = project.runtimeFeatures.cache.enabled
      || Object.keys(project.runtimeFeatures.cache.entrypoints).length > 0
      || project.runtimeFeatures.images !== undefined || project.runtimeFeatures.ai !== undefined
      || project.runtimeFeatures.versionMetadata !== undefined;
    if (projectExplicit && JSON.stringify(generatedRuntimeFeatures) !== JSON.stringify(project.runtimeFeatures)) {
      throw new Error("generated framework runtime features conflict with the project");
    }
  }
  const outputRoot = await realpath(dirname(configPath));
  const main = await outputPath(projectRoot, outputRoot, config.main, "generated framework main");
  let assets: AssetsProject | undefined;
  let assetsDirectory: string | undefined;
  if (config.assets !== undefined) {
    if (!record(config.assets)) throw new Error("generated framework assets config is invalid");
    assetsDirectory = await outputPath(projectRoot, outputRoot, config.assets.directory, "generated framework assets directory");
    if (within(assetsDirectory, main)) throw new Error("framework server entry is inside the client asset output");
    const relativeAssets = relative(projectRoot, assetsDirectory);
    assets = routing(config.assets, relativeAssets.split(sep).join("/"));
  }
  const worker = await importModules(outputRoot, main, assetsDirectory, moduleRules(config.rules));
  return { worker, ...(assets === undefined ? {} : { assets }), services,
    runtimeFeatures: generatedRuntimeFeatures ?? project.runtimeFeatures };
}

/** Overlay imported framework assets, services, and runtime features onto the validated project. */
export function applyFrameworkOutput(project: WorkerProject, framework: FrameworkOutput): WorkerProject {
  return {
    ...project,
    ...(framework.assets === undefined ? {} : { assets: framework.assets }),
    services: framework.services,
    runtimeFeatures: framework.runtimeFeatures,
  };
}
