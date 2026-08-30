import { WorkerEntrypoint } from "cloudflare:workers";
import { bytes, modulesFor } from "./modules.js";
export { modulesFor } from "./modules.js";
import { handleWorkflow } from "../workflows/host.js";
import { tenantEnv } from "./bindings.js";
import { routeDefaultHttp } from "../assets/router.js";
export { tenantEnv } from "./bindings.js";
export { WorkflowBindingTransport } from "../workflows/binding.js";
import { makeR2TransportBase } from "../r2/transport.js";
import { makeD1TransportBase } from "../d1/transport.js";
import type {
  AssetBindingProps, BindingEnv, ResourceBindingProps,
} from "../bindings/protocol.js";
import type { QueueBindingProps } from "../queues/protocol.js";
import type { AlarmIdentity, AlarmProjection, DoWireValue } from "../durable-objects/protocol.js";
import type { DispatchEnvelope, LoaderEnv, RuntimeModule } from "./protocol.js";
import {
  assembleOnce, bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration,
  doPolicy, INTERNAL_HEADERS, PROFILE, resolveSnapshot, TOKEN_HEADER,
} from "./shared.js";
export {
  bindingError, currentStartupGeneration, doPolicy, PROFILE, resolveSnapshot,
} from "./shared.js";
export { ServiceTransport } from "../services/transport.js";
export { CacheTransport } from "../cache/host.js";
export { ImageTransport } from "../images/host.js";

const MAX_QUEUE_MESSAGES = 100;
const MAX_QUEUE_BODY_BYTES = 128 * 1024;
const MAX_QUEUE_BATCH_BYTES = 256 * 1024;
const seenHashes = new Map<string, string>();

function stableError(code: string, status: number, requestId?: string | null): Response {
  return Response.json({
    ok: false,
    error: { code, message: "worker request failed", requestId: requestId || null },
  }, { status });
}

function classify(error: unknown): [string, number] {
  const message = String(error instanceof Error ? error.message : error);
  const service = [
    ["SERVICE_BINDING_DENIED", 403],
    ["SERVICE_TARGET_NOT_READY", 503],
    ["SERVICE_UNAVAILABLE", 503],
    ["SERVICE_ENTRYPOINT_NOT_FOUND", 404],
    ["SERVICE_LIMIT_EXCEEDED", 429],
    ["SERVICE_TIMEOUT", 504],
  ] as const;
  for (const [code, status] of service) {
    if (message.includes(code)) return [code, status];
  }
  if (/entrypoint|no such entrypoint|was not found/i.test(message)) {
    return ["ENTRYPOINT_NOT_FOUND", 404];
  }
  if (/limit|cpu time|subrequest/i.test(message)) {
    return ["RESOURCE_LIMIT_EXCEEDED", 429];
  }
  if (/syntax|parse|unexpected|module|wasm|initializ|startup/i.test(message)) {
    return ["BUNDLE_RUNTIME_INVALID", 422];
  }
  return ["RUNTIME_INTERNAL", 500];
}

function assertEnvelope(request: Request, validation: boolean, entrypointName: string | undefined): DispatchEnvelope {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  if (parts.length !== 3 || parts.some((part) => !/^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(part))) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected)) throw new Error("invalid descriptor hash");
  if (entrypointName && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)) {
    throw new Error("invalid entrypoint");
  }
  const routeGeneration = Number(request.headers.get("x-open-compute-route-generation"));
  if (!Number.isSafeInteger(routeGeneration)
      || (validation ? routeGeneration < 0 : routeGeneration < 1)) {
    throw new Error("invalid route generation");
  }
  return {
    loaderKey,
    expected,
    routeGeneration,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}/${expected}/g/${routeGeneration}/${entrypointName || "default"}`,
  };
}

export { KVNamespace } from "../kv/transport.js";

const R2TransportBase = makeR2TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class R2Transport extends R2TransportBase {}

const D1TransportBase = makeD1TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class D1Transport extends D1TransportBase {}

export class QueueTransport extends WorkerEntrypoint<BindingEnv, QueueBindingProps> {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.bindingId !== "string"
        || typeof props.deploymentId !== "string" || typeof props.queueId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.queueLifecycleGeneration)
        || props.queueLifecycleGeneration < 1) {
      throw bindingError("QUEUE_INVARIANT_VIOLATION");
    }
    return props;
  }

  async #request(operation: string, body?: BodyInit): Promise<unknown> {
    const props = this.#props();
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/queue/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": body === undefined
            ? "application/json"
            : "application/vnd.open-compute.queue.v1+frame",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-deployment-id": props.deploymentId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-request-id": crypto.randomUUID(),
        },
        ...(body === undefined ? {} : { body }),
      },
    );
    if (!response.ok) {
      const code = response.headers.get("x-open-compute-error-code")
        || "QUEUE_STORAGE_UNAVAILABLE";
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(code);
    }
    const result: unknown = await response.json();
    if (!result || typeof result !== "object") throw bindingError("QUEUE_INVARIANT_VIOLATION");
    return result;
  }

  send(frame: Uint8Array) {
    return this.#request("send", frame);
  }

  sendBatch(frame: Uint8Array) {
    return this.#request("batch", frame);
  }

  metrics() {
    return this.#request("metrics");
  }
}

export class AssetTransport extends WorkerEntrypoint<BindingEnv, AssetBindingProps> {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.deploymentId !== "string"
        || typeof props.descriptorSha256 !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) {
      throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    }
    return props;
  }

  async fetchAsset(input: unknown): Promise<Response> {
    if (!isRecord(input) || typeof input.url !== "string" || typeof input.method !== "string"
        || !Array.isArray(input.headers) || input.headers.length > 256) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
    const headers = new Headers();
    for (const pair of input.headers) {
      if (!Array.isArray(pair) || pair.length !== 2
          || typeof pair[0] !== "string" || typeof pair[1] !== "string") {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      headers.append(pair[0], pair[1]);
    }
    return this.#fetch(new Request(input.url, { method: input.method, headers }));
  }

  fetch(request: Request): Promise<Response> {
    return this.#fetch(request);
  }

  async #fetch(request: Request): Promise<Response> {
    const props = this.#props();
    const headers = new Headers(request.headers);
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    headers.set(BINDING_TOKEN_HEADER, this.env.BINDING_BACKEND_TOKEN);
    headers.set("x-open-compute-startup-generation", currentStartupGeneration());
    headers.set("x-open-compute-deployment-id", props.deploymentId);
    headers.set("x-open-compute-descriptor-sha256", props.descriptorSha256);
    headers.set("x-open-compute-request-id", crypto.randomUUID());
    headers.set("x-open-compute-asset-method", request.method);
    headers.set("x-open-compute-asset-url", request.url);
    const response = await this.env.BINDING_BACKEND.fetch(
      "http://binding-backend/internal/assets/v1/fetch",
      { method: "POST", headers, redirect: "manual" },
    );
    if (!response.ok && response.headers.has("x-open-compute-error-code")) {
      throw bindingError(response.headers.get("x-open-compute-error-code") || "ASSET_STORAGE_UNAVAILABLE");
    }
    const responseHeaders = new Headers(response.headers);
    const representationLength = responseHeaders.get("x-open-compute-asset-representation-length")
      ?? responseHeaders.get("content-length");
    if (representationLength) {
      responseHeaders.set("x-open-compute-asset-representation-length", representationLength);
    }
    const forwarded = new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: responseHeaders,
    });
    if (representationLength) forwarded.headers.set("content-length", representationLength);
    return forwarded;
  }
}

export class DoTransport extends WorkerEntrypoint<LoaderEnv, ResourceBindingProps> {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.accountId !== "string" || typeof props.workerId !== "string"
        || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string"
        || typeof props.namespaceResourceId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.routeGeneration) || props.routeGeneration < 1) {
      throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    }
    return props;
  }

  #headers(objectId: string) {
    const props = this.#props();
    if (typeof objectId !== "string" || !/^[0-9a-f]{64}$/.test(objectId)) {
      throw bindingError("DO_ID_INVALID");
    }
    return {
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-account-id": props.accountId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-binding-id": props.bindingId,
      "x-open-compute-deployment-id": props.deploymentId,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-route-generation": String(props.routeGeneration),
      "x-open-compute-namespace-resource-id": props.namespaceResourceId,
      "x-open-compute-object-id": objectId,
      "x-open-compute-request-id": crypto.randomUUID(),
    };
  }

  async fetch(request: Request): Promise<Response> {
    const objectId = new URL(request.url).pathname.slice(1);
    const headers = new Headers(request.headers);
    const tenantMethod = headers.get("x-open-compute-do-method") || request.method;
    const tenantUrl = headers.get("x-open-compute-do-url") || "https://do.invalid/";
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    for (const [name, value] of Object.entries(this.#headers(objectId))) headers.set(name, value);
    headers.set("x-open-compute-do-method", tenantMethod);
    headers.set("x-open-compute-do-url", tenantUrl);
    headers.set("x-open-compute-do-operation", "fetch");
    const init: RequestInit = { method: request.method, headers, body: request.body, redirect: "manual" };
    if (request.method === "GET" || request.method === "HEAD") delete init.body;
    return this.env.DO_ROUTER.fetch(new Request(
      "http://do-router/internal/do/v1/fetch",
      init,
    ));
  }

  async dispatchRpc(objectId: string, method: string, args: DoWireValue): Promise<unknown> {
    const response = await this.env.DO_ROUTER.fetch(
      "http://do-router/internal/do/v1/rpc",
      {
        method: "POST",
        headers: {
          ...this.#headers(objectId),
          "content-type": "application/json",
          "x-open-compute-do-operation": "rpc",
        },
        body: JSON.stringify({ method, args }),
      },
    );
    if (!response.ok) {
      throw bindingError(response.headers.get("x-open-compute-error-code") || "DO_RUNTIME_EXCEPTION");
    }
    const payload: unknown = await response.json();
    if (!isRecord(payload)) throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
    return payload.value;
  }
}

export class AlarmIndex extends WorkerEntrypoint<BindingEnv, AlarmIdentity> {
  #props() {
    const props = this.ctx.props;
    if (!props || typeof props.namespaceResourceId !== "string"
        || typeof props.objectId !== "string" || !/^[0-9a-f]{64}$/.test(props.objectId)
        || !Number.isSafeInteger(props.objectGeneration) || props.objectGeneration < 1) {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    return props;
  }

  async #request(operation: string, mutation: AlarmProjection | { rowToken: string } | Record<string, never> = {}) {
    const props = this.#props();
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/alarms/v1/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-request-id": crypto.randomUUID(),
        },
        body: JSON.stringify({
          namespaceResourceId: props.namespaceResourceId,
          objectId: props.objectId,
          objectGeneration: props.objectGeneration,
          ...mutation,
        }),
      },
    );
    if (!response.ok) {
      throw bindingError(response.headers.get("x-open-compute-error-code")
        || "DO_ALARM_INDEX_UNAVAILABLE");
    }
  }

  async upsert(row: AlarmProjection) {
    if (!row || !Number.isSafeInteger(row.scheduledTimeMs) || row.scheduledTimeMs <= 0
        || !Number.isSafeInteger(row.retryCount) || row.retryCount < 0 || row.retryCount > 6
        || typeof row.rowToken !== "string") {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    await this.#request("upsert", row);
  }

  async delete(rowToken: string) {
    if (typeof rowToken !== "string") throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    await this.#request("delete", { rowToken });
  }

  async clear() {
    await this.#request("clear");
  }
}

function tenantRequest(request: Request): Request {
  const headers = new Headers(request.headers);
  const method = request.headers.get("x-open-compute-original-method") || "GET";
  const url = request.headers.get("x-open-compute-original-url") || "https://worker.invalid/";
  for (const name of INTERNAL_HEADERS) headers.delete(name);
  const init: RequestInit = { method, headers, body: request.body, redirect: "manual" };
  if (method === "GET" || method === "HEAD") delete init.body;
  return new Request(url, init);
}

export class OutboundGateway extends WorkerEntrypoint<Record<string, never>, { deploymentId: string; policyVersion: 1 }> {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError("OUTBOUND_DENIED");
    }
    return fetch(new Request(request, { redirect: "follow" }));
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

export function stableCode(error: unknown): string | undefined {
  return isRecord(error) && typeof error.stableCode === "string" ? error.stableCode : undefined;
}

async function handle(request: Request, env: LoaderEnv, ctx: ExecutionContext, validation: boolean) {
  const requestId = request.headers.get("x-open-compute-request-id") || crypto.randomUUID();
  let executionStarted = false;
  try {
    const entrypoint = request.headers.get("x-open-compute-entrypoint") || undefined;
    const envelope = assertEnvelope(request, validation, entrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(env, envelope, validation, Boolean(entrypoint), internalToken);
    const deploymentId = envelope.loaderKey.split("/")[2]!;
    const tenant = validation ? undefined : tenantRequest(request);
    if (!validation && !entrypoint && tenant && routeDefaultHttp(snapshot, tenant) === "asset") {
      const response = await ctx.exports.AssetTransport({ props: Object.freeze({
        deploymentId,
        descriptorSha256: snapshot.workerCodeSha256,
      }) }).fetch(tenant);
      const headers = new Headers(response.headers);
      const representationLength = headers.get("x-open-compute-asset-representation-length")
        ?? headers.get("content-length");
      for (const name of INTERNAL_HEADERS) headers.delete(name);
      if (representationLength) {
        headers.set("x-open-compute-asset-representation-length", representationLength);
      }
      headers.set("x-open-compute-request-id", requestId);
      headers.set("x-open-compute-loader-outcome", "asset");
      const forwarded = new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
      if (representationLength) forwarded.headers.set("content-length", representationLength);
      return forwarded;
    }
    if (snapshot.contentKind !== "worker") throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    const prior = seenHashes.get(envelope.runtimeKey);
    if (prior && prior !== snapshot.workerCodeSha256) {
      throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
    }
    seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
    const code = await assembleOnce(envelope.runtimeKey, async () => {
      const built = modulesFor(snapshot, validation, entrypoint);
      return {
        compatibilityDate: snapshot.compatibilityDate,
        compatibilityFlags: snapshot.compatibilityFlags,
        mainModule: built.mainModule,
        modules: built.modules,
        env: validation ? {} : tenantEnv(
          snapshot, ctx, deploymentId, doPolicy(env), false, true, entrypoint ?? "default",
        ),
        globalOutbound: validation ? null : ctx.exports.OutboundGateway({
          props: { deploymentId, policyVersion: 1 },
        }),
        limits: PROFILE,
      };
    });
    let cold = false;
    const stub = env.LOADER.get(envelope.runtimeKey, async () => {
      cold = true;
      return code;
    });
    const target = stub.getEntrypoint(validation ? undefined : entrypoint, { limits: PROFILE });
    executionStarted = !validation;
    const response = await target.fetch(validation ? "https://validation.invalid/" : tenant!);
    if (validation) {
      const body = await response.text();
      if (response.status !== 200 || body !== "open-compute-validation-v1") {
        throw new Error("validation nonce mismatch");
      }
      return new Response(null, { status: 204 });
    }
    const headers = new Headers(response.headers);
    const representationLength = headers.get("x-open-compute-asset-representation-length");
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    if (representationLength) {
      headers.set("x-open-compute-asset-representation-length", representationLength);
    }
    headers.set("x-open-compute-request-id", requestId);
    headers.set("x-open-compute-loader-outcome", cold ? "cold" : "warm");
    if (executionStarted) headers.set("x-open-compute-execution-started", "1");
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  } catch (error) {
    const stable = stableCode(error);
    if (stable) {
      const status = stable === "DEPLOYMENT_NOT_READY" ? 409
        : stable === "ARTIFACT_UNAVAILABLE" ? 503
        : stable === "BUNDLE_RUNTIME_INVALID" ? 422
        : 500;
      const response = stableError(stable, status, requestId);
      if (executionStarted) response.headers.set("x-open-compute-execution-started", "1");
      return response;
    }
    const [code, status] = classify(error);
    const response = stableError(code, status, requestId);
    if (executionStarted) response.headers.set("x-open-compute-execution-started", "1");
    return response;
  }
}

function customEventMessageBody(message: Record<string, unknown>) {
  if (!message || typeof message !== "object"
      || typeof message.bodyBase64 !== "string") {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  const raw = bytes(message.bodyBase64);
  if (raw.byteLength > MAX_QUEUE_BODY_BYTES) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  let body: unknown;
  switch (message.contentType) {
    case "json":
      body = JSON.parse(new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw));
      break;
    case "text":
      body = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw);
      break;
    case "bytes":
      body = raw;
      break;
    default:
      throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  return { body, byteLength: raw.byteLength };
}

async function customEventTarget(request: Request, env: LoaderEnv, ctx: ExecutionContext) {
  const entrypoint = request.headers.get("x-open-compute-entrypoint") || undefined;
  const envelope = assertEnvelope(request, false, entrypoint);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(env, envelope, false, Boolean(entrypoint), internalToken);
  const prior = seenHashes.get(envelope.runtimeKey);
  if (prior && prior !== snapshot.workerCodeSha256) {
    throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
  }
  seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
  const code = await assembleOnce(envelope.runtimeKey, async () => {
    const built = modulesFor(snapshot, false, entrypoint);
    const deploymentId = envelope.loaderKey.split("/")[2]!;
    return {
      compatibilityDate: snapshot.compatibilityDate,
      compatibilityFlags: snapshot.compatibilityFlags,
      mainModule: built.mainModule,
      modules: built.modules,
      env: tenantEnv(
        snapshot, ctx, deploymentId, doPolicy(env), false, true, entrypoint ?? "default",
      ),
      globalOutbound: ctx.exports.OutboundGateway({
        props: { deploymentId, policyVersion: 1 },
      }),
      limits: PROFILE,
    };
  });
  let cold = false;
  const stub = env.LOADER.get(envelope.runtimeKey, async () => {
    cold = true;
    return code;
  });
  return {
    target: stub.getEntrypoint(entrypoint, { limits: PROFILE }),
    loaderOutcome: () => cold ? "cold" : "warm",
  };
}

async function handleQueue(request: Request, env: LoaderEnv, ctx: ExecutionContext) {
  try {
    const payload: unknown = await request.json();
    if (!isRecord(payload)
        || typeof payload.queueName !== "string" || payload.queueName.length < 1
        || payload.queueName.length > 128 || !Array.isArray(payload.messages)
        || payload.messages.length < 1 || payload.messages.length > MAX_QUEUE_MESSAGES) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    let totalBytes = 0;
    const messages = payload.messages.map((message: unknown) => {
      if (!isRecord(message) || typeof message.id !== "string"
          || typeof message.timestampMs !== "number" || !Number.isSafeInteger(message.timestampMs) || message.timestampMs < 0
          || typeof message.attempts !== "number" || !Number.isSafeInteger(message.attempts)
          || message.attempts < 1 || message.attempts > 101) {
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      }
      const decoded = customEventMessageBody(message);
      totalBytes += decoded.byteLength;
      if (totalBytes > MAX_QUEUE_BATCH_BYTES) {
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      }
      return {
        id: message.id,
        timestamp: new Date(message.timestampMs),
        attempts: message.attempts,
        body: decoded.body,
      };
    });
    const loaded = await customEventTarget(request, env, ctx);
    const result = await loaded.target.queue(payload.queueName, messages);
    const response = Response.json(result);
    response.headers.set("x-open-compute-loader-outcome", loaded.loaderOutcome());
    return response;
  } catch (error) {
    const stable = stableCode(error);
    return stableError(stable || "QUEUE_CUSTOM_EVENT_UNSUPPORTED", stable ? 422 : 500, null);
  }
}

async function handleScheduled(request: Request, env: LoaderEnv, ctx: ExecutionContext) {
  try {
    const payload: unknown = await request.json();
    if (!isRecord(payload)
        || typeof payload.scheduledTimeMs !== "number" || !Number.isSafeInteger(payload.scheduledTimeMs) || payload.scheduledTimeMs < 0
        || typeof payload.cron !== "string" || payload.cron.length < 1
        || payload.cron.length > 256) {
      throw bindingError("CRON_EXPRESSION_INVALID");
    }
    const loaded = await customEventTarget(request, env, ctx);
    const result = await loaded.target.scheduled({
      scheduledTime: new Date(payload.scheduledTimeMs),
      cron: payload.cron,
    });
    const response = Response.json(result);
    response.headers.set("x-open-compute-loader-outcome", loaded.loaderOutcome());
    return response;
  } catch (error) {
    const stable = stableCode(error);
    return stableError(stable || "CRON_CUSTOM_EVENT_UNSUPPORTED", stable ? 422 : 500, null);
  }
}

function moduleExportsDurableObjectClass(modules: readonly RuntimeModule[], className: string): boolean {
  const patterns = [
    new RegExp(`export\\s+class\\s+${className}\\b`),
    new RegExp(`export\\s+(?:const|let|var)\\s+${className}\\s*=\\s*class\\b`),
    new RegExp(`export\\s*\\{[^}]*\\b${className}\\b[^}]*\\}`),
  ];
  return modules.some((module) => {
    if (module.type !== "esModule") return false;
    const source = new TextDecoder().decode(bytes(module.bytesBase64));
    return patterns.some((pattern) => pattern.test(source));
  });
}

async function validateDurableObjectClass(request: Request, env: LoaderEnv) {
  const className = request.headers.get("x-open-compute-entrypoint") || "";
  const envelope = assertEnvelope(request, true, className);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(env, envelope, true, false, internalToken);
  if (!moduleExportsDurableObjectClass(snapshot.modules, className)) {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
  const built = modulesFor(snapshot, false, className, true);
  const code = {
    compatibilityDate: snapshot.compatibilityDate,
    compatibilityFlags: snapshot.compatibilityFlags,
    mainModule: built.mainModule,
    modules: built.modules,
    env: {},
    globalOutbound: null,
    limits: PROFILE,
  };
  try {
    const loaded = env.LOADER.get(`validate-do/${envelope.runtimeKey}`, () => code);
    loaded.getDurableObjectClass(className);
    return new Response(null, { status: 204 });
  } catch {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
}

export default {
  async fetch(request: Request, env: LoaderEnv, ctx: ExecutionContext): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (request.method === "POST" && ["/internal/workflow", "/internal/validate-workflow"].includes(path)) {
      return handleWorkflow(request, env, ctx, path === "/internal/validate-workflow");
    }
    if (request.method === "POST" && path === "/internal/dispatch") return handle(request, env, ctx, false);
    if (request.method === "POST" && path === "/internal/queue") {
      return handleQueue(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/scheduled") {
      return handleScheduled(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/validate") return handle(request, env, ctx, true);
    if (request.method === "POST" && path === "/internal/validate-do") {
      return validateDurableObjectClass(request, env);
    }
    return new Response(null, { status: 404 });
  },
};
