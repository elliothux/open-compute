import { open } from "node:fs/promises";
import { dirname, resolve } from "node:path";

/** JSON values admitted in public Worker variables. */
export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };

/** A reference to an existing resource, validated again by the Rust authority. */
export interface WorkerBinding {
  type: "kv_namespace" | "r2_bucket" | "d1_database" | "do_namespace" | "queue_producer" | "workflow";
  id: string;
  permissions?: { read: boolean; write: boolean };
}

/** Developer project configuration. Secret values never belong in this document. */
export interface WorkerProject {
  readonly project: string;
  readonly main: string;
  readonly name: string;
  readonly tsconfig: string;
  readonly compatibilityDate: string;
  readonly compatibilityFlags: string[];
  readonly vars: Record<string, JsonValue>;
  readonly secrets: Record<string, { env: string }>;
  readonly bindings: Record<string, WorkerBinding>;
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
  knownKeys(value, ["main", "name", "tsconfig", "compatibilityDate", "compatibilityFlags", "vars", "secrets", "bindings", "accountId", "endpoint"], "project");
  const name = string(value.name, "name");
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(name)) throw new Error("invalid Worker name");
  const compatibilityDate = string(value.compatibilityDate, "compatibilityDate");
  if (!/^\d{4}-\d{2}-\d{2}$/.test(compatibilityDate)) throw new Error("invalid compatibility date");
  const flags: unknown = value.compatibilityFlags === undefined ? [] : value.compatibilityFlags;
  if (!Array.isArray(flags) || !flags.every((item): item is string => typeof item === "string")) throw new Error("invalid compatibility flags");
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
      knownKeys(item, ["type", "id", "permissions"], "binding");
      const kind = item.type;
      if (kind !== "kv_namespace" && kind !== "r2_bucket" && kind !== "d1_database" && kind !== "do_namespace" && kind !== "queue_producer" && kind !== "workflow") {
        throw new Error("unsupported Worker binding type");
      }
      const binding: WorkerBinding = { type: kind, id: string(item.id, "binding id") };
      if (item.permissions !== undefined) {
        if (!record(item.permissions) || typeof item.permissions.read !== "boolean" || typeof item.permissions.write !== "boolean") throw new Error("invalid binding permissions");
        knownKeys(item.permissions, ["read", "write"], "binding permissions");
        binding.permissions = { read: item.permissions.read, write: item.permissions.write };
      }
      Object.defineProperty(bindings, key, { value: binding, enumerable: true });
    }
  }
  return {
    project: dirname(filename), main: string(value.main, "main"), name,
    tsconfig: value.tsconfig === undefined ? "tsconfig.json" : string(value.tsconfig, "tsconfig"),
    compatibilityDate, compatibilityFlags: flags, vars: variables, secrets, bindings,
    ...(value.accountId === undefined ? {} : { accountId: string(value.accountId, "accountId") }),
    endpoint: value.endpoint === undefined ? "http://127.0.0.1:8787" : string(value.endpoint, "endpoint"),
  };
}
