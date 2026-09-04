import type { LoaderEnv, RuntimeObservabilityIdentity } from "../loader/protocol.js";
import { currentStartupGeneration } from "../loader/shared.js";

const TOKEN_HEADER = "x-open-compute-observability-token";
const MAX_BATCH = 128;
const MAX_LOGS = 1024;
const MAX_EXCEPTIONS = 256;
const MAX_DEPTH = 32;
const MAX_LEAVES = 256;
const MAX_STRING = 16_384;
const MAX_ENVELOPE_BYTES = 256 * 1024;
const MAX_PROJECT_BYTES = 192 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

interface ProjectBudget { leaves: number; bytes: number; truncated: boolean }

function projectedString(value: string, budget: ProjectBudget, maximum = MAX_STRING): string {
  const encoded = encoder.encode(value);
  const allowed = Math.min(maximum, budget.bytes);
  if (encoded.byteLength <= allowed) {
    budget.bytes -= encoded.byteLength;
    return value;
  }
  budget.truncated = true;
  let end = allowed;
  while (end > 0 && end < encoded.byteLength && (encoded[end]! & 0xc0) === 0x80) end -= 1;
  const output = decoder.decode(encoded.slice(0, end));
  budget.bytes = 0;
  return output;
}

function dataProperties(value: object): readonly [string, unknown][] {
  const output: [string, unknown][] = [];
  for (const [key, descriptor] of Object.entries(Object.getOwnPropertyDescriptors(value))) {
    if ("value" in descriptor && typeof descriptor.value !== "function" && typeof descriptor.value !== "symbol") {
      output.push([key, descriptor.value]);
    }
  }
  return output;
}

function projected(value: unknown, budget: ProjectBudget, depth = 0): unknown {
  if (value === null || typeof value === "boolean") return value;
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (budget.bytes === 0) { budget.truncated = true; return null; }
  if (typeof value === "string") return projectedString(value, budget);
  if (value instanceof Date) return value.getTime();
  if (depth >= MAX_DEPTH || budget.leaves >= MAX_LEAVES) {
    budget.truncated = true;
    return null;
  }
  if (Array.isArray(value)) {
    const output: unknown[] = [];
    for (const item of value.slice(0, MAX_LEAVES - budget.leaves)) {
      if (budget.bytes === 0) { budget.truncated = true; break; }
      budget.leaves += 1;
      output.push(projected(item, budget, depth + 1));
    }
    if (output.length !== value.length) budget.truncated = true;
    return output;
  }
  if (typeof value === "object") {
    const output: Record<string, unknown> = {};
    for (const [key, item] of dataProperties(value)) {
      if (budget.leaves >= MAX_LEAVES || budget.bytes === 0) { budget.truncated = true; break; }
      budget.leaves += 1;
      output[projectedString(key, budget, 512)] = projected(item, budget, depth + 1);
    }
    return output;
  }
  return null;
}

function secretHeader(name: string): boolean {
  const lower = name.toLowerCase();
  return lower === "cookie" || lower === "set-cookie"
    || ["auth", "key", "secret", "token", "jwt"].some(part => lower.includes(part))
    || lower.startsWith("x-open-compute-");
}

function redactUrl(raw: string): string {
  try {
    const url = new URL(raw);
    url.username = "";
    url.password = "";
    for (const key of [...url.searchParams.keys()]) {
      if (secretHeader(key)) url.searchParams.set(key, "REDACTED");
    }
    url.pathname = url.pathname.replace(/[A-Za-z0-9_-]{32,}|[0-9a-fA-F]{24,}/g, "REDACTED");
    return url.toString();
  } catch {
    return "https://redacted.invalid/";
  }
}

function traceEvent(value: TraceItem["event"], budget: ProjectBudget): unknown {
  const event = projected(value, budget);
  if (!event || typeof event !== "object" || Array.isArray(event)) return event;
  const output = event as Record<string, unknown>;
  const request = output.request;
  if (request && typeof request === "object" && !Array.isArray(request)) {
    const projectedRequest = request as Record<string, unknown>;
    if (typeof projectedRequest.url === "string") projectedRequest.url = redactUrl(projectedRequest.url);
    const headers = projectedRequest.headers;
    if (headers && typeof headers === "object" && !Array.isArray(headers)) {
      const projectedHeaders = headers as Record<string, unknown>;
      for (const [name, headerValue] of Object.entries(projectedHeaders)) {
        if (secretHeader(name) || typeof headerValue !== "string") projectedHeaders[name] = "REDACTED";
      }
    }
    delete projectedRequest.cf;
    delete projectedRequest.getUnredacted;
  }
  return output;
}

function traceItem(item: TraceItem): Record<string, unknown> {
  const budget: ProjectBudget = { leaves: 0, bytes: MAX_PROJECT_BYTES, truncated: item.truncated };
  const logs = item.logs.slice(0, MAX_LOGS).map(log => ({
    level: projectedString(String(log.level), budget, 32),
    message: projected(log.message as unknown, budget),
    timestamp: Number.isFinite(log.timestamp) ? log.timestamp : null,
  }));
  const exceptions = item.exceptions.slice(0, MAX_EXCEPTIONS).map(exception => ({
    name: projectedString(String(exception.name), budget, 256),
    message: projectedString(String(exception.message), budget),
    ...(exception.stack === undefined ? {} : { stack: projectedString(String(exception.stack), budget) }),
    timestamp: Number.isFinite(exception.timestamp) ? exception.timestamp : null,
  }));
  if (logs.length !== item.logs.length || exceptions.length !== item.exceptions.length) {
    budget.truncated = true;
  }
  return {
    outcome: String(item.outcome).slice(0, 64),
    scriptName: item.scriptName,
    exceptions,
    logs,
    eventTimestamp: item.eventTimestamp,
    event: traceEvent(item.event, budget),
    ...(item.entrypoint === undefined ? {} : { entrypoint: projectedString(String(item.entrypoint), budget, 256) }),
    ...(item.scriptVersion === undefined ? {} : { scriptVersion: projected(item.scriptVersion, budget) }),
    executionModel: item.executionModel,
    truncated: budget.truncated,
    cpuTime: Number.isFinite(item.cpuTime) ? item.cpuTime : 0,
    wallTime: Number.isFinite(item.wallTime) ? item.wallTime : 0,
    ...(item.durableObjectId === undefined ? {} : {
      durableObjectId: projectedString(String(item.durableObjectId), budget, 512),
    }),
  };
}

function validIdentity(value: RuntimeObservabilityIdentity): boolean {
  return value.schemaVersion === 1
    && /^[0-9a-f-]{36}$/.test(value.accountId)
    && /^[0-9a-f-]{36}$/.test(value.workerId)
    && /^[0-9a-f-]{36}$/.test(value.versionId)
    && value.scriptName.length > 0 && value.scriptName.length <= 63
    && Number.isSafeInteger(value.routeGeneration) && value.routeGeneration > 0
    && Number.isSafeInteger(value.observabilityGeneration) && value.observabilityGeneration > 0;
}

/** Collect one platform-owned tail batch through the generation-authenticated backend. */
export async function collectObservabilityTail(
  events: TraceItem[],
  env: LoaderEnv,
  identity: RuntimeObservabilityIdentity,
): Promise<void> {
  try {
    if (!validIdentity(identity) || events.length === 0 || events.length > MAX_BATCH) return;
    const envelope: Record<string, unknown> = {
      schemaVersion: 1,
      collectorEventId: crypto.randomUUID(),
      identity: { ...identity },
      items: events.map(traceItem),
    };
    let bytes = encoder.encode(JSON.stringify(envelope));
    while (bytes.byteLength > MAX_ENVELOPE_BYTES && Array.isArray(envelope.items)
      && envelope.items.length > 1) {
      envelope.items.pop();
      envelope.batchTruncated = true;
      bytes = encoder.encode(JSON.stringify(envelope));
    }
    if (bytes.byteLength > MAX_ENVELOPE_BYTES) return;
    await env.OBSERVABILITY_BACKEND.fetch("http://observability-backend/internal/observability/v1/ingest", {
      method: "POST",
      headers: {
        "content-type": "application/json",
        [TOKEN_HEADER]: env.OBSERVABILITY_BACKEND_TOKEN,
        "x-open-compute-startup-generation": currentStartupGeneration(),
      },
      body: bytes,
    });
  } catch {
    // Projection and delivery are best-effort and cannot change the completed tenant invocation.
  }
}

/** Construct the platform tail service through the statically exported RPC namespace. */
export function attachObservabilityTail(
  ctx: { readonly exports: ExecutionContext["exports"] },
  identity: RuntimeObservabilityIdentity,
): Fetcher {
  const exports = ctx.exports as unknown as {
    ObservabilityTail(options: { props: Readonly<RuntimeObservabilityIdentity> }): Fetcher;
  };
  return exports.ObservabilityTail({ props: Object.freeze({ ...identity }) });
}

/** Attach exactly one platform collector to a tenant execution target. */
export function collectableWorkerCode(
  code: WorkerLoaderWorkerCode,
  ctx: { readonly exports: ExecutionContext["exports"] },
  identity: RuntimeObservabilityIdentity | undefined,
): WorkerLoaderWorkerCode {
  if (identity === undefined) return code;
  return { ...code, tails: [attachObservabilityTail(ctx, identity)] };
}
