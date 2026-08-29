import { constants } from "node:fs";
import { open, opendir, realpath } from "node:fs/promises";
import { dirname, extname, isAbsolute, relative, resolve, sep } from "node:path";
import type { CompiledModule, CompiledModuleType, CompiledWorker } from "../build-worker.ts";
import type { WorkerProject, WorkerService } from "../project.ts";
import { record } from "../project.ts";
import type { AssetsProject } from "../assets/types.ts";

const MAX_CONFIG_BYTES = 256 * 1024;
const MAX_MODULES = 128;
const MAX_MODULE_BYTES = 4 * 1024 * 1024;
const MAX_TOTAL_MODULE_BYTES = 16 * 1024 * 1024;
const GENERATED_BINDING_KEYS = [
  "vars", "define", "durable_objects", "kv_namespaces", "d1_databases", "r2_buckets",
  "queues", "workflows", "dispatch_namespaces", "ai", "vectorize",
  "hyperdrive", "browser", "images", "mtls_certificates", "version_metadata", "unsafe",
] as const;

interface ModuleRule {
  readonly type: CompiledModuleType;
  readonly globs: readonly RegExp[];
  readonly fallthrough: boolean;
}

/** Immutable framework build imported from a generated Wrangler deployment description. */
export interface FrameworkOutput {
  readonly worker: CompiledWorker;
  readonly assets?: AssetsProject;
  readonly services: Record<string, WorkerService>;
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

function outputPath(root: string, base: string, value: unknown, label: string): Promise<string> {
  const path = resolve(base, string(value, label));
  if (!within(root, path)) throw new Error(`${label} must stay inside the project`);
  return realpath(path).then(actual => {
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
      if (pattern[index + 1] === "*") { source += ".*"; index += 1; }
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
    if (!rule.fallthrough) break;
  }
  switch (extname(name).toLowerCase()) {
    case ".js": case ".mjs": return "esModule";
    case ".cjs": return "commonJsModule";
    case ".json": return "json";
    case ".wasm": return "wasm";
    case ".txt": return "text";
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
  configPath: string,
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
        if (filename === configPath || logical.endsWith(".map")) continue;
        const type = classify(logical, rules);
        if (type === undefined) throw new Error(`framework server output type is unsupported: ${extname(logical) || "extensionless"}`);
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
  onlyKeys(redirect, ["configPath"], "framework deploy config");
  const configPath = await outputPath(projectRoot, dirname(redirectPath), redirect.configPath, "generated framework configPath");
  const config = await boundedJson(configPath, "generated framework config");
  const generatedName = config.name;
  if (generatedName !== undefined && generatedName !== project.name) throw new Error("generated framework Worker name does not match the project");
  if (config.compatibility_date !== project.compatibilityDate) throw new Error("generated framework compatibility date does not match the project");
  const generatedFlags = config.compatibility_flags ?? [];
  if (!Array.isArray(generatedFlags) || !generatedFlags.every((item): item is string => typeof item === "string")
      || [...generatedFlags].sort().join("\0") !== [...project.compatibilityFlags].sort().join("\0")) {
    throw new Error("generated framework compatibility flags do not match the project");
  }
  if (GENERATED_BINDING_KEYS.some(key => config[key] !== undefined)) {
    throw new Error("generated framework config declares bindings that open-compute cannot import yet");
  }
  const services = { ...project.services };
  for (const [binding, declaration] of Object.entries(serviceDeclarations(config.services))) {
    const existing = services[binding];
    if (existing !== undefined
        && (existing.service !== declaration.service || existing.entrypoint !== declaration.entrypoint)) {
      throw new Error("generated framework service conflicts with the project");
    }
    Object.defineProperty(services, binding, { value: declaration, enumerable: true });
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
  const worker = await importModules(outputRoot, configPath, main, assetsDirectory, moduleRules(config.rules));
  return { worker, ...(assets === undefined ? {} : { assets }), services };
}
