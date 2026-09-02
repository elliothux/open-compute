import { createRequire } from "node:module";
import { dirname, relative, resolve, sep } from "node:path";
import type { AssetsProject } from "./assets/types.ts";

interface NormalizedWranglerConfig {
  configPath?: string;
  userConfigPath?: string;
  name?: string;
  account_id?: string;
  main?: string;
  tsconfig?: string;
  vars: Record<string, JsonValue>;
  secrets?: { required?: string[] };
  kv_namespaces: { binding: string; id?: string }[];
  r2_buckets: { binding: string; bucket_name?: string }[];
  d1_databases: { binding: string; database_id?: string; database_name?: string }[];
  durable_objects: { bindings: { name: string; class_name: string; script_name?: string }[] };
  queues: { producers?: { binding: string; queue?: string }[] };
  workflows: { binding: string; name: string; class_name: string; schedules?: string | string[] }[];
  vectorize: { binding: string; index_name: string }[];
  ai_search_namespaces: { binding: string; namespace: string }[];
  ai_search: { binding: string; instance_name: string }[];
  services?: { binding: string; service: string; entrypoint?: string; environment?: string; props?: unknown; remote?: boolean }[];
  assets?: {
    directory?: string;
    binding?: string;
    run_worker_first?: boolean | string[];
    html_handling?: AssetsProject["htmlHandling"];
    not_found_handling?: AssetsProject["notFoundHandling"];
  };
  [key: string]: unknown;
}

type ReadWranglerConfig = (
  args: { config: string } | { script: string },
  options: { hideWarnings: boolean; preserveOriginalMain: boolean; useRedirectIfAvailable: boolean },
) => NormalizedWranglerConfig;
type ReadRawWranglerConfig = (args: { config: string } | { script: string }, options: { useRedirectIfAvailable: boolean }) => {
  configPath?: string;
  userConfigPath?: string;
  deployConfigPath?: string;
  redirected: boolean;
  rawConfig: unknown;
};

const require = createRequire(import.meta.url);
const wranglerModule: unknown = require("wrangler");
if (!record(wranglerModule) || typeof wranglerModule.unstable_readConfig !== "function"
    || typeof wranglerModule.experimental_readRawConfig !== "function") {
  throw new Error("pinned Wrangler does not export its config readers");
}
const readWranglerConfig = wranglerModule.unstable_readConfig as ReadWranglerConfig;
const readRawWranglerConfig = wranglerModule.experimental_readRawConfig as ReadRawWranglerConfig;

/** JSON values admitted by Wrangler for public Worker variables. */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/** A normalized binding used only by local build and type-generation code. */
export interface WorkerBinding {
  type: "kv_namespace" | "r2_bucket" | "d1_database" | "do_namespace" | "queue_producer" | "workflow"
    | "vectorize_index" | "ai_search_namespace" | "ai_search_instance";
  id: string;
  className?: string;
  schedules?: string[];
}

/** A normalized standard Wrangler service binding. */
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
  ai?: { binding: string };
  versionMetadata?: { binding: string };
}

/** Local build projection of Wrangler's normalized configuration. */
export interface WorkerProject {
  readonly project: string;
  readonly configPath: string;
  readonly main?: string;
  readonly frameworkOutput?: string;
  readonly name: string;
  readonly tsconfig: string;
  readonly vars: Record<string, JsonValue>;
  readonly secrets: readonly string[];
  readonly bindings: Record<string, WorkerBinding>;
  readonly services: Record<string, WorkerService>;
  readonly runtimeFeatures: RuntimeFeatures;
  readonly assets?: AssetsProject;
  readonly accountId?: string;
}

/** Narrow untrusted JSON objects before reading generated framework fields. */
export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function relativeProjectPath(project: string, value: string, label: string): string {
  const path = relative(project, resolve(project, value));
  if (path === ".." || path.startsWith(`..${sep}`)) throw new Error(`${label} must be inside the project`);
  return path || ".";
}

/** Project runtime-only features from a Wrangler-normalized configuration. */
export function parseRuntimeFeatures(
  value: Record<string, unknown>,
  occupied: ReadonlySet<string> = new Set(),
): RuntimeFeatures {
  const cacheValue = record(value.cache) ? value.cache : {};
  const enabled = cacheValue.enabled === true;
  const crossVersionCache = cacheValue.cross_version_cache === true;
  const entrypoints: Record<string, { enabled: boolean; crossVersionCache: boolean }> = {};
  if (record(value.exports)) {
    for (const [name, exported] of Object.entries(value.exports)) {
      if (record(exported) && exported.type === "worker") {
        const exportCache = record(exported.cache) ? exported.cache : {};
        entrypoints[name] = {
          enabled: exportCache.enabled === undefined ? enabled : exportCache.enabled === true,
          crossVersionCache,
        };
      }
    }
  }
  const binding = (candidate: unknown, label: string): { binding: string } | undefined => {
    if (candidate === undefined) return undefined;
    if (!record(candidate) || typeof candidate.binding !== "string" || occupied.has(candidate.binding)) {
      throw new Error(`${label} binding conflicts with another environment name`);
    }
    return { binding: candidate.binding };
  };
  const images = binding(value.images, "images");
  const ai = binding(value.ai, "AI");
  const versionMetadata = binding(value.version_metadata, "version metadata");
  const names = [images?.binding, ai?.binding, versionMetadata?.binding]
    .filter((name): name is string => name !== undefined);
  if (new Set(names).size !== names.length) throw new Error("platform binding names conflict");
  return {
    cache: { enabled, crossVersionCache, entrypoints },
    ...(images === undefined ? {} : { images }),
    ...(ai === undefined ? {} : { ai }),
    ...(versionMetadata === undefined ? {} : { versionMetadata }),
  };
}

function addBinding(bindings: Record<string, WorkerBinding>, name: string, binding: WorkerBinding): void {
  if (Object.hasOwn(bindings, name)) throw new Error(`duplicate Wrangler binding ${name}`);
  Object.defineProperty(bindings, name, { enumerable: true, value: binding });
}

function normalizeBindings(config: NormalizedWranglerConfig): Record<string, WorkerBinding> {
  const bindings: Record<string, WorkerBinding> = {};
  for (const item of config.kv_namespaces) {
    addBinding(bindings, item.binding, { type: "kv_namespace", id: item.id ?? item.binding });
  }
  for (const item of config.r2_buckets) {
    addBinding(bindings, item.binding, { type: "r2_bucket", id: item.bucket_name ?? item.binding });
  }
  for (const item of config.d1_databases) {
    addBinding(bindings, item.binding, { type: "d1_database", id: item.database_id ?? item.database_name ?? item.binding });
  }
  for (const item of config.durable_objects.bindings) {
    addBinding(bindings, item.name, {
      type: "do_namespace",
      id: item.script_name ?? config.name ?? item.class_name,
      className: item.class_name,
    });
  }
  for (const item of config.queues.producers ?? []) {
    addBinding(bindings, item.binding, { type: "queue_producer", id: item.queue ?? item.binding });
  }
  for (const item of config.workflows) {
    const schedules = item.schedules === undefined
      ? undefined : typeof item.schedules === "string" ? [item.schedules] : item.schedules;
    addBinding(bindings, item.binding, {
      type: "workflow",
      id: item.name,
      className: item.class_name,
      ...(schedules === undefined ? {} : { schedules }),
    });
  }
  for (const item of config.vectorize) {
    addBinding(bindings, item.binding, { type: "vectorize_index", id: item.index_name });
  }
  for (const item of config.ai_search_namespaces) {
    addBinding(bindings, item.binding, { type: "ai_search_namespace", id: item.namespace });
  }
  for (const item of config.ai_search) {
    addBinding(bindings, item.binding, { type: "ai_search_instance", id: item.instance_name });
  }
  return bindings;
}

function normalizeServices(config: NormalizedWranglerConfig): Record<string, WorkerService> {
  const services: Record<string, WorkerService> = {};
  for (const item of config.services ?? []) {
    if (item.environment !== undefined || item.props !== undefined || item.remote !== undefined) {
      throw new Error("service environment, props, and remote selectors are unsupported by the local adapter");
    }
    if (Object.hasOwn(services, item.binding)) throw new Error(`duplicate Wrangler binding ${item.binding}`);
    Object.defineProperty(services, item.binding, {
      enumerable: true,
      value: { service: item.service, ...(item.entrypoint === undefined ? {} : { entrypoint: item.entrypoint }) },
    });
  }
  return services;
}

function normalizeAssets(project: string, assets: NormalizedWranglerConfig["assets"]): AssetsProject | undefined {
  if (assets?.directory === undefined) return undefined;
  return {
    directory: relativeProjectPath(project, assets.directory, "assets directory"),
    ...(assets.binding === undefined ? {} : { binding: assets.binding }),
    runWorkerFirst: assets.run_worker_first ?? false,
    htmlHandling: assets.html_handling ?? "auto-trailing-slash",
    notFoundHandling: assets.not_found_handling ?? "none",
    publishSourceMaps: false,
  };
}

/** Load a project through the exact pinned Wrangler schema and environment resolver. */
export async function loadProject(path: string): Promise<WorkerProject> {
  const requestedPath = resolve(path);
  const explicitRaw = readRawWranglerConfig({ config: requestedPath }, { useRedirectIfAvailable: true });
  const explicitConfig = readWranglerConfig(
    { config: requestedPath },
    { hideWarnings: true, preserveOriginalMain: true, useRedirectIfAvailable: true },
  );
  // Wrangler deliberately bypasses framework redirects for an explicit config.
  // Discover the standard redirect only when Wrangler resolves the same user
  // config that the caller selected; a sibling config can never take over.
  const discoveredRaw = readRawWranglerConfig({ script: requestedPath }, { useRedirectIfAvailable: true });
  const usesSelectedRedirect = discoveredRaw.redirected
    && discoveredRaw.userConfigPath !== undefined
    && resolve(discoveredRaw.userConfigPath) === requestedPath;
  const raw = usesSelectedRedirect ? discoveredRaw : explicitRaw;
  const config = usesSelectedRedirect
    ? readWranglerConfig(
      { script: requestedPath },
      { hideWarnings: true, preserveOriginalMain: true, useRedirectIfAvailable: true },
    )
    : explicitConfig;
  if (config.configPath === undefined || config.name === undefined) {
    throw new Error("Wrangler config requires a Worker name");
  }
  const userConfigPath = resolve(raw.userConfigPath ?? requestedPath);
  const project = dirname(userConfigPath);
  const frameworkOutput = raw.redirected && raw.deployConfigPath !== undefined
    ? relativeProjectPath(project, raw.deployConfigPath, "framework output config") : undefined;
  const main = config.main === undefined || frameworkOutput !== undefined
    ? undefined : relativeProjectPath(project, config.main, "Worker main");
  const assets = normalizeAssets(project, config.assets);
  if (main === undefined && assets === undefined && frameworkOutput === undefined) {
    throw new Error("Wrangler config requires main, assets, or a generated deployment config");
  }
  const vars = config.vars as Record<string, JsonValue>;
  const bindings = normalizeBindings(config);
  const services = normalizeServices(config);
  const occupied = new Set([
    ...Object.keys(vars), ...(config.secrets?.required ?? []), ...Object.keys(bindings), ...Object.keys(services),
    ...(assets?.binding === undefined ? [] : [assets.binding]),
  ]);
  return {
    project,
    configPath: relativeProjectPath(project, userConfigPath, "Wrangler config"),
    ...(main === undefined ? {} : { main }),
    ...(frameworkOutput === undefined ? {} : { frameworkOutput }),
    name: config.name,
    tsconfig: relativeProjectPath(project, config.tsconfig ?? "tsconfig.json", "tsconfig"),
    vars,
    secrets: [...(config.secrets?.required ?? [])],
    bindings,
    services,
    runtimeFeatures: parseRuntimeFeatures(config as unknown as Record<string, unknown>, occupied),
    ...(assets === undefined ? {} : { assets }),
    ...(config.account_id === undefined ? {} : { accountId: config.account_id }),
  };
}
