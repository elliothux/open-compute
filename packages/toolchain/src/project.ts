import { open } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import type { AssetsProject } from "./assets/types.ts";

/** JSON values admitted in public Worker variables. */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/** A reference to an existing resource, validated again by the Rust authority. */
export interface WorkerBinding {
  type: "kv_namespace" | "r2_bucket" | "d1_database" | "do_namespace" | "queue_producer" | "workflow";
  id: string;
  className?: string;
  permissions?: { read: boolean; write: boolean };
  schedules?: string[];
}

/** A Service target name resolved to one immutable Worker ID during deployment. */
export interface WorkerService {
  service: string;
  entrypoint?: string;
}

export interface RuntimeFeatures {
  cache: {
    enabled: boolean;
    crossVersionCache: boolean;
    entrypoints: Record<string, { enabled: boolean; crossVersionCache: boolean }>;
  };
  images?: { binding: string };
  versionMetadata?: { binding: string; tag?: string };
}

/** Developer project configuration. Secret values never belong in this document. */
export interface WorkerProject {
  readonly project: string;
  readonly main?: string;
  readonly frameworkOutput?: string;
  readonly name: string;
  readonly tsconfig: string;
  readonly vars: Record<string, JsonValue>;
  readonly secrets: Record<string, { env: string }>;
  readonly bindings: Record<string, WorkerBinding>;
  readonly services: Record<string, WorkerService>;
  readonly runtimeFeatures: RuntimeFeatures;
  readonly assets?: AssetsProject;
  readonly accountId?: string;
  readonly endpoint: string;
}

/** Narrow untrusted JSON objects before reading protocol fields. */
export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function string(value: unknown, label: string): string {
  if (typeof value !== "string" || value.length === 0) throw new Error(`invalid project ${label}`);
  return value;
}

function json(value: unknown, depth = 0): value is JsonValue {
  if (depth > 128) return false;
  if (value === null || typeof value === "boolean" || typeof value === "string") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(item => json(item, depth + 1));
  return record(value) && Object.values(value).every(item => json(item, depth + 1));
}

function knownKeys(value: Record<string, unknown>, allowed: readonly string[], label: string): void {
  if (Object.keys(value).some(key => !allowed.includes(key))) throw new Error(`unknown ${label} field`);
}

function cachePolicy(value: unknown, inherited?: { enabled: boolean; crossVersionCache: boolean }) {
  if (value === undefined) return inherited ?? { enabled: false, crossVersionCache: false };
  if (!record(value)) throw new Error("invalid cache policy");
  knownKeys(value, ["enabled", "cross_version_cache"], "cache policy");
  const enabled = value.enabled ?? inherited?.enabled ?? false;
  const crossVersionCache = value.cross_version_cache ?? inherited?.crossVersionCache ?? false;
  if (typeof enabled !== "boolean" || typeof crossVersionCache !== "boolean") throw new Error("invalid cache policy");
  return { enabled, crossVersionCache };
}

/** Parse the one shared runtime-feature grammar used by projects and framework output. */
export function parseRuntimeFeatures(
  value: Record<string, unknown>,
  occupied: ReadonlySet<string> = new Set(),
): RuntimeFeatures {
  let defaultPolicy = cachePolicy(value.cache);
  const entrypoints: Record<string, { enabled: boolean; crossVersionCache: boolean }> = {};
  if (value.exports !== undefined) {
    if (!record(value.exports) || Object.keys(value.exports).length > 128) throw new Error("invalid Worker exports");
    for (const [name, raw] of Object.entries(value.exports)) {
      if (!record(raw)) throw new Error("invalid Worker export");
      knownKeys(raw, ["type", "cache"], "Worker export");
      if (raw.type !== "worker" || !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(name)) {
        throw new Error("invalid Worker export");
      }
      const policy = cachePolicy(raw.cache, defaultPolicy);
      if (name === "default") defaultPolicy = policy;
      else Object.defineProperty(entrypoints, name, { value: policy, enumerable: true });
    }
  }
  const binding = (raw: unknown, label: string, tag = false) => {
    if (raw === undefined) return undefined;
    if (!record(raw)) throw new Error(`invalid ${label}`);
    knownKeys(raw, tag ? ["binding", "tag"] : ["binding"], label);
    const name = string(raw.binding, `${label} binding`);
    if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(name) || occupied.has(name)) {
      throw new Error(`${label} binding conflicts with another environment name`);
    }
    if (tag && raw.tag !== undefined && (typeof raw.tag !== "string" || !raw.tag.length
        || raw.tag.length > 128 || /[\u0000-\u001f\u007f]/.test(raw.tag))) throw new Error("invalid version metadata tag");
    return { binding: name, ...(tag && raw.tag !== undefined ? { tag: raw.tag as string } : {}) };
  };
  const images = binding(value.images, "images");
  const versionMetadata = binding(value.version_metadata, "version metadata", true);
  if (images && versionMetadata && images.binding === versionMetadata.binding) {
    throw new Error("platform binding names conflict");
  }
  return {
    cache: { ...defaultPolicy, entrypoints },
    ...(images === undefined ? {} : { images }),
    ...(versionMetadata === undefined ? {} : { versionMetadata }),
  };
}

/** Load bounded JSON without evaluating a project-supplied configuration module. */
export async function loadProject(path: string): Promise<WorkerProject> {
  const filename = resolve(path);
  const file = await open(filename, "r");
  let content: string;
  try {
    const info = await file.stat();
    if (!info.isFile() || info.size > 64 * 1024) throw new Error("project config must be a regular file of at most 64 KiB");
    const bytes = Buffer.alloc(64 * 1024 + 1);
    let length = 0;
    while (length < bytes.length) {
      const { bytesRead } = await file.read(bytes, length, bytes.length - length, null);
      if (!bytesRead) break;
      length += bytesRead;
    }
    if (length > 64 * 1024) throw new Error("project config exceeds 64 KiB");
    content = new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(0, length));
  } finally { await file.close(); }
  let value: unknown;
  try { value = JSON.parse(content); }
  catch { throw new Error("project config must be valid JSON"); }
  if (!record(value)) throw new Error("project config must be an object");
  knownKeys(value, ["main", "frameworkOutput", "name", "tsconfig", "vars", "secrets", "bindings", "services", "assets", "cache", "exports", "images", "version_metadata", "accountId", "endpoint"], "project");
  const name = string(value.name, "name");
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name)) throw new Error("invalid Worker name");
  const vars = value.vars === undefined ? {} : value.vars;
  if (!record(vars)) throw new Error("invalid Worker vars");
  const variables: Record<string, JsonValue> = {};
  for (const [key, item] of Object.entries(vars)) {
    if (!json(item)) throw new Error("invalid Worker variable");
    Object.defineProperty(variables, key, { value: item, enumerable: true });
  }
  const secrets: Record<string, { env: string }> = {};
  if (value.secrets !== undefined) {
    if (!record(value.secrets)) throw new Error("secrets must contain environment references");
    for (const [key, reference] of Object.entries(value.secrets)) {
      if (!record(reference) || typeof reference.env !== "string" || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(reference.env)) {
        throw new Error("secrets must contain environment references");
      }
      knownKeys(reference, ["env"], "secret reference");
      Object.defineProperty(secrets, key, { value: { env: reference.env }, enumerable: true });
    }
  }
  const bindings: Record<string, WorkerBinding> = {};
  if (value.bindings !== undefined) {
    if (!record(value.bindings)) throw new Error("invalid Worker bindings");
    for (const [key, item] of Object.entries(value.bindings)) {
      if (!record(item)) throw new Error("invalid Worker binding");
      knownKeys(item, ["type", "id", "className", "permissions", "schedules"], "binding");
      const kind = item.type;
      if (kind !== "kv_namespace" && kind !== "r2_bucket" && kind !== "d1_database" && kind !== "do_namespace" && kind !== "queue_producer" && kind !== "workflow") {
        throw new Error("unsupported Worker binding type");
      }
      const binding: WorkerBinding = { type: kind, id: string(item.id, "binding id") };
      if (item.className !== undefined) {
        if ((kind !== "do_namespace" && kind !== "workflow")
            || typeof item.className !== "string"
            || !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(item.className)) {
          throw new Error("invalid binding className");
        }
        binding.className = item.className;
      } else if (kind === "do_namespace" || kind === "workflow") {
        throw new Error("class-bound Worker binding requires className");
      }
      if (item.schedules !== undefined) {
        if (kind !== "workflow" || !Array.isArray(item.schedules) || item.schedules.length > 100
            || !item.schedules.every(value => typeof value === "string" && value.length >= 1
              && value.length <= 256)) {
          throw new Error("invalid Workflow schedules");
        }
        binding.schedules = [...new Set(item.schedules)].sort();
      }
      if (item.permissions !== undefined) {
        if (!record(item.permissions) || typeof item.permissions.read !== "boolean" || typeof item.permissions.write !== "boolean") throw new Error("invalid binding permissions");
        knownKeys(item.permissions, ["read", "write"], "binding permissions");
        binding.permissions = { read: item.permissions.read, write: item.permissions.write };
      }
      Object.defineProperty(bindings, key, { value: binding, enumerable: true });
    }
  }
  const services: Record<string, WorkerService> = {};
  if (value.services !== undefined) {
    if (!Array.isArray(value.services) || value.services.length > 64) {
      throw new Error("invalid Worker services");
    }
    for (const item of value.services) {
      if (!record(item)) throw new Error("invalid Worker service");
      knownKeys(item, ["binding", "service", "entrypoint"], "service");
      const binding = string(item.binding, "service binding");
      const service = string(item.service, "service target");
      if (!/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(binding)
          || !/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(service)) {
        throw new Error("invalid Worker service");
      }
      const entrypoint = item.entrypoint === undefined
        ? undefined : string(item.entrypoint, "service entrypoint");
      if (entrypoint !== undefined && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypoint)) {
        throw new Error("invalid Worker service entrypoint");
      }
      if (Object.hasOwn(services, binding)) throw new Error("duplicate Worker service binding");
      if (Object.hasOwn(variables, binding) || Object.hasOwn(secrets, binding)
          || Object.hasOwn(bindings, binding)) {
        throw new Error("service binding conflicts with another environment name");
      }
      Object.defineProperty(services, binding, {
        value: { service, ...(entrypoint === undefined ? {} : { entrypoint }) },
        enumerable: true,
      });
    }
  }
  let assets: AssetsProject | undefined;
  if (value.assets !== undefined) {
    if (!record(value.assets)) throw new Error("invalid project assets");
    knownKeys(value.assets, ["directory", "binding", "run_worker_first", "html_handling", "not_found_handling", "publish_source_maps"], "assets");
    const runWorkerFirst = value.assets.run_worker_first === undefined ? false : value.assets.run_worker_first;
    if (typeof runWorkerFirst !== "boolean"
        && (!Array.isArray(runWorkerFirst) || !runWorkerFirst.length
          || runWorkerFirst.length > 100 || !runWorkerFirst.every((rule): rule is string =>
            typeof rule === "string" && rule.length > 0 && rule.length <= 2048
            && (rule.startsWith("/") || rule.startsWith("!/"))))) {
      throw new Error("invalid assets runWorkerFirst");
    }
    const htmlHandling = value.assets.html_handling ?? "auto-trailing-slash";
    if (!["auto-trailing-slash", "force-trailing-slash", "drop-trailing-slash", "none"].includes(String(htmlHandling))) {
      throw new Error("invalid assets htmlHandling");
    }
    const notFoundHandling = value.assets.not_found_handling ?? "none";
    if (!["none", "404-page", "single-page-application"].includes(String(notFoundHandling))) {
      throw new Error("invalid assets notFoundHandling");
    }
    const binding = value.assets.binding;
    if (binding !== undefined && (typeof binding !== "string" || !/^[A-Za-z_][A-Za-z0-9_]{0,63}$/.test(binding))) {
      throw new Error("invalid assets binding");
    }
    if (binding !== undefined && (Object.hasOwn(variables, binding) || Object.hasOwn(secrets, binding)
        || Object.hasOwn(bindings, binding) || Object.hasOwn(services, binding))) {
      throw new Error("assets binding conflicts with another environment name");
    }
    if (value.assets.publish_source_maps !== undefined && typeof value.assets.publish_source_maps !== "boolean") {
      throw new Error("invalid assets publishSourceMaps");
    }
    assets = {
      directory: string(value.assets.directory, "assets directory"),
      ...(binding === undefined ? {} : { binding }),
      runWorkerFirst,
      htmlHandling: htmlHandling as AssetsProject["htmlHandling"],
      notFoundHandling: notFoundHandling as AssetsProject["notFoundHandling"],
      publishSourceMaps: value.assets.publish_source_maps === true,
    };
  }
  const runtimeFeatures = parseRuntimeFeatures(value, new Set([
    ...Object.keys(variables), ...Object.keys(secrets), ...Object.keys(bindings),
    ...Object.keys(services), ...(assets?.binding === undefined ? [] : [assets.binding]),
  ]));
  const main = value.main === undefined ? undefined : string(value.main, "main");
  const frameworkOutput = value.frameworkOutput === undefined
    ? undefined : string(value.frameworkOutput, "frameworkOutput");
  if (frameworkOutput !== undefined && (main !== undefined || assets !== undefined)) {
    throw new Error("frameworkOutput cannot be combined with main or assets");
  }
  if (main === undefined && assets === undefined && frameworkOutput === undefined) {
    throw new Error("project requires main, assets, or frameworkOutput");
  }
  if (main === undefined && assets !== undefined
      && (Object.keys(variables).length || Object.keys(secrets).length || Object.keys(bindings).length
        || Object.keys(services).length
        || runtimeFeatures.cache.enabled || Object.keys(runtimeFeatures.cache.entrypoints).length
        || runtimeFeatures.images !== undefined || runtimeFeatures.versionMetadata !== undefined
        || runWorkerFirstRequiresCode(assets.runWorkerFirst))) {
    throw new Error("assets-only projects cannot declare an execution environment");
  }
  return {
    project: dirname(filename), ...(main === undefined ? {} : { main }),
    ...(frameworkOutput === undefined ? {} : { frameworkOutput }), name,
    tsconfig: value.tsconfig === undefined ? "tsconfig.json" : string(value.tsconfig, "tsconfig"),
    vars: variables, secrets, bindings, services,
    runtimeFeatures,
    ...(assets === undefined ? {} : { assets }),
    ...(value.accountId === undefined ? {} : { accountId: string(value.accountId, "accountId") }),
    endpoint: value.endpoint === undefined ? "http://127.0.0.1:8787" : string(value.endpoint, "endpoint"),
  };
}

function runWorkerFirstRequiresCode(value: boolean | readonly string[]): boolean {
  return value === true || Array.isArray(value);
}
