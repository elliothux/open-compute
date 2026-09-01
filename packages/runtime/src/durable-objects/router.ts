export { DoHost } from "./host.js";
import { WorkerEntrypoint } from "cloudflare:workers";
import { currentStartupGeneration, doPolicy, stableCode } from "../loader/host.js";
import {
  inboundSocketAddress,
  tunnelSockets,
  validateSocketAuthorityWire,
  type SocketAuthorityWire,
} from "../sockets/tunnel.js";
import type { DoHostEnv, DoOrder, DoPolicy, DoPolicyEnv, ResolvedDoAuthority } from "./protocol.js";
export {
  AlarmIndex,
  AssetTransport,
  CacheTransport,
  D1Transport,
  DoTransport,
  KVNamespace,
  ImageTransport,
  QueueTransport,
  R2Transport,
  ServiceTransport,
  WorkflowBindingTransport,
} from "../loader/host.js";

const TOKEN_HEADER = "x-open-compute-binding-token";
const ERROR_HEADER = "x-open-compute-error-code";
const FORBIDDEN_RPC = new Set([
  "constructor", "prototype", "__proto__", "then", "dup", "fetch", "connect", "alarm",
  "webSocketMessage", "webSocketClose", "webSocketError",
]);
let activeDispatches = 0;
const pendingConnects = new Map<string, {
  expiresAt: number;
  hostKey: string;
  tokenAddress: string;
}>();

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

function orderFromHeaders(headers: Headers): DoOrder {
  const channelId = value(
    headers,
    "x-open-compute-do-order-channel",
    /^[0-9a-f]{32}$/,
  );
  const sequence = Number(headers.get("x-open-compute-do-order-sequence"));
  if (!Number.isSafeInteger(sequence) || sequence < 0) {
    throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return { channelId, sequence };
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
      /^(connect|fetch|rpc)$/,
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

function assertRpcMember(member: unknown): asserts member is string {
  if (typeof member !== "string" || FORBIDDEN_RPC.has(member)
      || member.startsWith("__openCompute")) {
    throw stableFailure("DO_RPC_UNSUPPORTED");
  }
}

async function dispatchNativeRpc(env: DoHostEnv, identity: Record<string, string>, method: string, args: unknown[]) {
  assertRpcMember(method);
  if (!Array.isArray(args)) throw stableFailure("DO_RPC_UNSUPPORTED");
  const request = new Request("http://do-router/internal/do/v1/rpc", {
    method: "POST",
    headers: identity,
  });
  const authority = await authorize(request, env);
  const value = await host(env, authority).dispatchTenantRpc(
    authority, orderFromHeaders(request.headers), method, args,
  );
  await acknowledge(request, env, authority);
  return value;
}

async function getNativeRpcProperty(env: DoHostEnv, identity: Record<string, string>, property: string) {
  assertRpcMember(property);
  const request = new Request("http://do-router/internal/do/v1/rpc", {
    method: "POST",
    headers: identity,
  });
  const authority = await authorize(request, env);
  const value = await host(env, authority).getTenantRpcProperty(
    authority, orderFromHeaders(request.headers), property,
  );
  await acknowledge(request, env, authority);
  return value;
}

async function prepareNativeConnect(
  env: DoHostEnv,
  identity: Record<string, string>,
  authorityWire: SocketAuthorityWire,
) {
  const connectAuthority = validateSocketAuthorityWire(authorityWire);
  const request = new Request("http://do-router/internal/do/v1/connect", {
    method: "POST",
    headers: identity,
  });
  const authority = await authorize(request, env);
  const target = host(env, authority);
  const token = await target.__openComputePrepareConnect(
    authority, orderFromHeaders(request.headers), connectAuthority,
  );
  await acknowledge(request, env, authority);
  const now = Date.now();
  for (const [handoff, pending] of pendingConnects) {
    if (pending.expiresAt <= now) pendingConnects.delete(handoff);
  }
  if (pendingConnects.size >= 1024) throw stableFailure("DO_STORAGE_LIMIT");
  const handoff = crypto.randomUUID().replaceAll("-", "");
  const expiresAt = now + 10_000;
  pendingConnects.set(handoff, {
    expiresAt,
    hostKey: authority.hostKey,
    tokenAddress: `${token}.do-connect.invalid:1`,
  });
  return { tokenAddress: `${handoff}.do-router.invalid:1` };
}

async function cancelNativeOrder(env: DoHostEnv, identity: Record<string, string>) {
  const request = new Request("http://do-router/internal/do/v1/rpc", {
    method: "POST",
    headers: identity,
  });
  const authority = await authorize(request, env);
  await host(env, authority).__openComputeCancelOrder(
    orderFromHeaders(request.headers),
  );
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

export default class DoRouter extends WorkerEntrypoint<DoHostEnv> {
  async dispatchFetch(identity: Record<string, string>, request: Request): Promise<Response> {
    if (!record(identity) || !(request instanceof Request)) {
      throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
    }
    const headers = new Headers(request.headers);
    for (const [name, value] of Object.entries(identity)) headers.set(name, value);
    headers.set("x-open-compute-do-method", request.method);
    headers.set("x-open-compute-do-url", request.url);
    headers.set("x-open-compute-do-operation", "fetch");
    const init: RequestInit = {
      method: request.method,
      headers,
      body: request.body,
      redirect: "manual",
    };
    if (request.method === "GET" || request.method === "HEAD") delete init.body;
    const internal = new Request("http://do-router/internal/do/v1/fetch", init);
    return admitted(this.env, policy => dispatchFetch(internal, this.env, policy));
  }

  async dispatchRpc(identity: Record<string, string>, method: string, args: unknown[]): Promise<unknown> {
    if (!record(identity)) throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
    return admitted(this.env, () => dispatchNativeRpc(this.env, identity, method, args));
  }

  async getRpcProperty(identity: Record<string, string>, property: string): Promise<unknown> {
    if (!record(identity)) throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
    return admitted(this.env, () => getNativeRpcProperty(this.env, identity, property));
  }


  prepareConnect(identity: Record<string, string>, authority: SocketAuthorityWire) {
    return admitted(this.env, () => prepareNativeConnect(this.env, identity, authority));
  }

  async cancelOrder(identity: Record<string, string>): Promise<void> {
    if (!record(identity)) throw stableFailure("DO_INTERNAL_PROTOCOL_ERROR");
    await admitted(this.env, () => cancelNativeOrder(this.env, identity));
  }

  async connect(socket: Socket): Promise<void> {
    try {
      const tokenAddress = await inboundSocketAddress(socket);
      const match = /^([0-9a-f]{32})\.do-router\.invalid:1$/.exec(tokenAddress);
      const pending = match ? pendingConnects.get(match[1]!) : undefined;
      if (!match || !pending || pending.expiresAt <= Date.now()) {
        if (match) pendingConnects.delete(match[1]!);
        throw stableFailure("DO_RUNTIME_EXCEPTION");
      }
      pendingConnects.delete(match[1]!);
      const target = host(this.env, { hostKey: pending.hostKey })
        .connect(pending.tokenAddress, { allowHalfOpen: true });
      await target.opened;
      await tunnelSockets(socket, target);
    } catch {
      await socket.close().catch(() => undefined);
      throw stableFailure("DO_RUNTIME_EXCEPTION");
    }
  }

  async fetch(request: Request): Promise<Response> {
    try {
      const path = new URL(request.url).pathname;
      if (path === "/internal/do/v1/fetch") {
        return await admitted(this.env, policy => dispatchFetch(request, this.env, policy));
      }
      if (request.method !== "POST") return error("DO_INTERNAL_PROTOCOL_ERROR", 405);
      if (path === "/internal/do/v1/delete") return deleteObject(request, this.env);
      if (path === "/internal/do-delete") return deleteAuthorized(request, this.env);
      if (path === "/internal/do-alarm") return admitted(this.env, () => dispatchAlarm(request, this.env, false));
      if (path === "/internal/do-alarm-repair") {
        return admitted(this.env, () => dispatchAlarm(request, this.env, true));
      }
      return error("DO_INTERNAL_PROTOCOL_ERROR", 404);
    } catch (failure) {
      return error(stableCode(failure) || "DO_RUNTIME_EXCEPTION");
    }
  }
}
