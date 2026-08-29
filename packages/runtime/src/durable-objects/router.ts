export { DoHost } from "./host.js";
import { currentStartupGeneration, doPolicy, stableCode } from "../loader/host.js";
import type { DoHostEnv, DoPolicy, DoPolicyEnv, ResolvedDoAuthority } from "./protocol.js";
export {
  AlarmIndex,
  AssetTransport,
  D1Transport,
  DoTransport,
  KVNamespace,
  OutboundGateway,
  QueueTransport,
  R2Transport,
  ServiceTransport,
  WorkflowBindingTransport,
} from "../loader/host.js";

const TOKEN_HEADER = "x-open-compute-binding-token";
const ERROR_HEADER = "x-open-compute-error-code";
const PUBLIC_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const FORBIDDEN_RPC = new Set(["constructor", "prototype", "__proto__", "then", "fetch"]);
let activeDispatches = 0;

function error(code: string, status = 500): Response {
  return new Response(null, {
    status,
    headers: { [ERROR_HEADER]: code },
  });
}

function value(headers: Headers, name: string, pattern: RegExp): string {
  const item = headers.get(name) || "";
  if (!pattern.test(item)) throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
  return item;
}

function stableFailure(code: string): Error & { stableCode: string } {
  return Object.assign(new Error(code), { stableCode: code });
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function assertAuthority(authority: unknown): asserts authority is ResolvedDoAuthority {
  if (!record(authority)) throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
  for (const name of ["accountId", "workerId", "deploymentId", "workerCodeSha256", "namespaceResourceId", "objectId", "className", "hostKey"]) {
    if (typeof authority[name] !== "string") throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
  }
  for (const name of ["objectGeneration", "routeGeneration"]) {
    const generation = authority[name];
    if (typeof generation !== "number" || !Number.isSafeInteger(generation) || generation < 1) {
      throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
    }
  }
}

function boundedBody(body: ReadableStream<Uint8Array> | null, maximum: number): ReadableStream<Uint8Array> | null {
  if (!body) return null;
  let observed = 0;
  return body.pipeThrough(new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      observed += chunk.byteLength;
      if (observed > maximum) throw stableFailure("DO_STORAGE_LIMIT");
      controller.enqueue(chunk);
    },
  }));
}

function admitted<T>(env: DoPolicyEnv, operation: (policy: DoPolicy) => Promise<T>): Promise<T> {
  const policy = doPolicy(env);
  if (activeDispatches >= policy.maxInFlightDispatches) {
    throw stableFailure("DO_STORAGE_LIMIT");
  }
  activeDispatches += 1;
  const pending = Promise.resolve()
    .then(() => operation(policy))
    .finally(() => { activeDispatches -= 1; });
  return Promise.race([
    pending,
    scheduler.wait(policy.dispatchTimeoutMs).then(() => {
      throw stableFailure("DO_DISPATCH_TIMEOUT");
    }),
  ]);
}

function backendHeaders(request: Request, env: DoHostEnv) {
  return {
    [TOKEN_HEADER]: env.BINDING_BACKEND_TOKEN,
    "x-open-compute-startup-generation": value(
      request.headers,
      "x-open-compute-startup-generation",
      /^[^\x00-\x1f]{1,128}$/,
    ),
    "x-open-compute-deployment-id": value(
      request.headers,
      "x-open-compute-deployment-id",
      /^[0-9a-f-]{36}$/,
    ),
    "x-open-compute-descriptor-sha256": value(
      request.headers,
      "x-open-compute-descriptor-sha256",
      /^[0-9a-f]{64}$/,
    ),
    "x-open-compute-route-generation": value(
      request.headers,
      "x-open-compute-route-generation",
      /^[1-9][0-9]{0,19}$/,
    ),
    "x-open-compute-request-id": value(
      request.headers,
      "x-open-compute-request-id",
      /^[0-9a-f-]{36}$/,
    ),
    "x-open-compute-do-operation": value(
      request.headers,
      "x-open-compute-do-operation",
      /^(fetch|rpc)$/,
    ),
    "content-type": "application/json",
  };
}

async function authorize(request: Request, env: DoHostEnv): Promise<ResolvedDoAuthority> {
  const bindingId = value(request.headers, "x-open-compute-binding-id", /^[0-9a-f-]{36}$/);
  const objectId = value(request.headers, "x-open-compute-object-id", /^[0-9a-f]{64}$/);
  const headers = backendHeaders(request, env);
  const response = await env.BINDING_BACKEND.fetch(
    `http://binding-backend/internal/bindings/v1/do/${bindingId}/resolve`,
    { method: "POST", headers, body: JSON.stringify({ objectId }) },
  );
  if (!response.ok) {
    throw stableFailure(response.headers.get(ERROR_HEADER) || "DO_STORAGE_UNAVAILABLE");
  }
  const authority: unknown = await response.json();
  assertAuthority(authority);
  return authority;
}

async function acknowledge(request: Request, env: DoHostEnv, authority: ResolvedDoAuthority) {
  const bindingId = value(request.headers, "x-open-compute-binding-id", /^[0-9a-f-]{36}$/);
  const response = await env.BINDING_BACKEND.fetch(
    `http://binding-backend/internal/bindings/v1/do/${bindingId}/ready`,
    {
      method: "POST",
      headers: backendHeaders(request, env),
      body: JSON.stringify({
        namespaceResourceId: authority.namespaceResourceId,
        objectId: authority.objectId,
        objectGeneration: authority.objectGeneration,
      }),
    },
  );
  if (!response.ok) {
    throw stableFailure(response.headers.get(ERROR_HEADER) || "DO_STORAGE_UNAVAILABLE");
  }
}

function hostHeaders(request: Request, authority: ResolvedDoAuthority): Headers {
  const headers = new Headers(request.headers);
  headers.set("x-open-compute-account-id", authority.accountId);
  headers.set("x-open-compute-worker-id", authority.workerId);
  headers.set("x-open-compute-deployment-id", authority.deploymentId);
  headers.set("x-open-compute-worker-code-sha256", authority.workerCodeSha256);
  headers.set("x-open-compute-route-generation", String(authority.routeGeneration));
  headers.set("x-open-compute-object-id", authority.objectId);
  headers.set("x-open-compute-object-generation", String(authority.objectGeneration));
  headers.set("x-open-compute-class-name", authority.className);
  headers.set("x-open-compute-namespace-resource-id", authority.namespaceResourceId);
  return headers;
}

function host(env: DoHostEnv, authority: Pick<ResolvedDoAuthority, "hostKey">) {
  return env.DO_HOST.get(env.DO_HOST.idFromName(authority.hostKey));
}

async function dispatchFetch(request: Request, env: DoHostEnv, policy: DoPolicy) {
  const declared = Number(request.headers.get("content-length") || 0);
  if (declared > policy.maxFetchBodyBytes) throw stableFailure("DO_STORAGE_LIMIT");
  const authority = await authorize(request, env);
  const init: RequestInit = {
    method: request.method,
    headers: hostHeaders(request, authority),
    body: boundedBody(request.body, policy.maxFetchBodyBytes),
    redirect: "manual",
  };
  if (request.method === "GET" || request.method === "HEAD") delete init.body;
  const response = await host(env, authority).fetch(new Request("http://do-host/internal/fetch", init));
  await acknowledge(request, env, authority);
  return response;
}

async function dispatchRpc(request: Request, env: DoHostEnv, policy: DoPolicy) {
  const declared = Number(request.headers.get("content-length") || 0);
  if (declared > policy.maxRpcRequestBytes) return error("DO_RPC_UNSUPPORTED", 413);
  const bytes = await request.arrayBuffer();
  if (bytes.byteLength > policy.maxRpcRequestBytes) return error("DO_RPC_UNSUPPORTED", 413);
  let payload: unknown;
  try { payload = JSON.parse(new TextDecoder().decode(bytes)); } catch { return error("DO_RPC_UNSUPPORTED", 400); }
  if (!record(payload) || typeof payload.method !== "string" || !PUBLIC_METHOD.test(payload.method) || FORBIDDEN_RPC.has(payload.method)
      || payload.method.startsWith("__openCompute")
      || !Array.isArray(payload.args)) return error("DO_RPC_UNSUPPORTED", 400);
  const authority = await authorize(request, env);
  const headers = hostHeaders(request, authority);
  headers.set("x-open-compute-do-operation", "rpc");
  headers.set("content-type", "application/json");
  const response = await host(env, authority).fetch(
    new Request("http://do-host/internal/rpc", {
      method: "POST",
      headers,
      body: JSON.stringify(payload),
    }),
  );
  await acknowledge(request, env, authority);
  const body = await response.arrayBuffer();
  if (body.byteLength > policy.maxRpcResponseBytes) {
    return error("DO_RPC_UNSUPPORTED", 413);
  }
  return new Response(body, {
    status: response.status,
    headers: { "content-type": "application/json" },
  });
}

async function deleteObject(request: Request, env: DoHostEnv) {
  const authority = await authorize(request, env);
  return deleteHost(env, authority);
}

async function deleteAuthorized(request: Request, env: DoHostEnv) {
  const authority: unknown = await request.json();
  if (!record(authority) || typeof authority.hostKey !== "string" || !/^[A-Za-z0-9_-]{43}$/.test(authority.hostKey)
      || typeof authority.objectId !== "string" || !/^[0-9a-f]{64}$/.test(authority.objectId)
      || typeof authority.objectGeneration !== "number" || !Number.isSafeInteger(authority.objectGeneration)) {
    return error("DO_INTERNAL_PROTOCOL_ERROR", 400);
  }
  return deleteHost(env, { hostKey: authority.hostKey, objectId: authority.objectId, objectGeneration: authority.objectGeneration });
}

async function alarmAuthority(request: Request, env: DoHostEnv) {
  const body: unknown = await request.json();
  if (!record(body) || typeof body.namespaceResourceId !== "string"
      || typeof body.objectId !== "string" || !/^[0-9a-f]{64}$/.test(body.objectId)
      || typeof body.objectGeneration !== "number" || !Number.isSafeInteger(body.objectGeneration) || body.objectGeneration < 1) {
    throw stableFailure("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
  }
  const response = await env.BINDING_BACKEND.fetch(
    "http://binding-backend/internal/alarms/v1/resolve",
    {
      method: "POST",
      headers: {
        [TOKEN_HEADER]: env.BINDING_BACKEND_TOKEN,
        "x-open-compute-startup-generation": currentStartupGeneration(env.INTERNAL_TOKEN),
        "x-open-compute-request-id": crypto.randomUUID(),
        "content-type": "application/json",
      },
      body: JSON.stringify({
        namespaceResourceId: body.namespaceResourceId,
        objectId: body.objectId,
        objectGeneration: body.objectGeneration,
      }),
    },
  );
  if (!response.ok) {
    throw stableFailure(response.headers.get(ERROR_HEADER) || "DO_OBJECT_DELETING");
  }
  const authority: unknown = await response.json();
  assertAuthority(authority);
  return { body, authority };
}

async function dispatchAlarm(request: Request, env: DoHostEnv, repair: boolean) {
  const { body, authority } = await alarmAuthority(request, env);
  const headers = hostHeaders(request, authority);
  headers.set("x-open-compute-do-operation", repair ? "alarm-repair" : "alarm");
  headers.set("content-type", "application/json");
  const payload = repair ? {} : { rowToken: body.rowToken, retryCount: body.retryCount };
  return host(env, authority).fetch(new Request(
    repair ? "http://do-host/internal/alarm-repair" : "http://do-host/internal/alarm",
    { method: "POST", headers, body: JSON.stringify(payload) },
  ));
}

function deleteHost(env: DoHostEnv, authority: Pick<ResolvedDoAuthority, "hostKey" | "objectId" | "objectGeneration">) {
  const headers = new Headers({
    "x-open-compute-object-id": authority.objectId,
    "x-open-compute-object-generation": String(authority.objectGeneration),
    "x-open-compute-do-operation": "delete",
  });
  return host(env, authority).fetch(new Request("http://do-host/internal/delete", {
    method: "POST",
    headers,
  }));
}

export default {
  async fetch(request: Request, env: DoHostEnv): Promise<Response> {
    try {
      const path = new URL(request.url).pathname;
      if (path === "/internal/do/v1/fetch") {
        return admitted(env, policy => dispatchFetch(request, env, policy));
      }
      if (request.method !== "POST") return error("DO_INTERNAL_PROTOCOL_ERROR", 405);
      if (path === "/internal/do/v1/rpc") {
        return admitted(env, policy => dispatchRpc(request, env, policy));
      }
      if (path === "/internal/do/v1/delete") return deleteObject(request, env);
      if (path === "/internal/do-delete") return deleteAuthorized(request, env);
      if (path === "/internal/do-alarm") return admitted(env, () => dispatchAlarm(request, env, false));
      if (path === "/internal/do-alarm-repair") {
        return admitted(env, () => dispatchAlarm(request, env, true));
      }
      return error("DO_INTERNAL_PROTOCOL_ERROR", 404);
    } catch (failure) {
      return error(stableCode(failure) || "DO_RUNTIME_EXCEPTION");
    }
  },
};
