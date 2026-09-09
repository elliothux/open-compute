import { routeDefaultHttp } from "../assets/router.js";
import { observedEntrypoint } from "../observability/collector.js";
import { decodeDurableValue } from "../serialization/codec.js";
import {
  SERVICE_WEBSOCKET_HANDOFF_HEADER,
  serviceWebSocketHandoffHandles,
} from "../services/facade.js";
import { tenantEnv } from "./bindings.js";
import { bytes, modulesFor } from "./modules.js";
import type { DispatchEnvelope, LoaderEnv, RuntimeModule } from "./protocol.js";
import {
  assembleOnce,
  bindingError,
  doPolicy,
  INTERNAL_HEADERS,
  isRecord,
  resolveSnapshot,
  snapshotWorkerCode,
  stableCode,
  tenantGlobalOutbound,
  TOKEN_HEADER,
} from "./shared.js";

const MAX_QUEUE_MESSAGES = 100;
const MAX_QUEUE_BODY_BYTES = 128 * 1024;
const MAX_QUEUE_BATCH_BYTES = 256 * 1024;
const SCHEDULED_WORKFLOW_BINDING = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;

function stableError(
  code: string,
  status: number,
  requestId?: string | null,
): Response {
  return Response.json(
    {
      ok: false,
      error: {
        code,
        message: "worker request failed",
        requestId: requestId || null,
      },
    },
    { status },
  );
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

function assertEnvelope(
  request: Request,
  validation: boolean,
  entrypointName: string | undefined,
): DispatchEnvelope {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected =
    request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  if (
    parts.length !== 3 ||
    parts.some((part) => !/^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(part))
  ) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected))
    throw new Error("invalid descriptor hash");
  if (
    entrypointName &&
    !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(entrypointName)
  ) {
    throw new Error("invalid entrypoint");
  }
  const routeGeneration = Number(
    request.headers.get("x-open-compute-route-generation"),
  );
  if (
    !Number.isSafeInteger(routeGeneration) ||
    (validation ? routeGeneration < 0 : routeGeneration < 1)
  ) {
    throw new Error("invalid route generation");
  }
  return {
    loaderKey,
    expected,
    routeGeneration,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}/${expected}/${entrypointName || "default"}`,
  };
}

function tenantRequest(request: Request): Request {
  const headers = new Headers(request.headers);
  const method = request.headers.get("x-open-compute-original-method") || "GET";
  const url =
    request.headers.get("x-open-compute-original-url") ||
    "https://worker.invalid/";
  for (const name of INTERNAL_HEADERS) headers.delete(name);
  const init: RequestInit = {
    method,
    headers,
    body: request.body,
    redirect: "manual",
  };
  if (method === "GET" || method === "HEAD") delete init.body;
  return new Request(url, init);
}

export async function handleDispatch(
  request: Request,
  env: LoaderEnv,
  ctx: ExecutionContext,
  validation: boolean,
) {
  const requestId =
    request.headers.get("x-open-compute-request-id") || crypto.randomUUID();
  let executionStarted = false;
  try {
    const entrypoint =
      request.headers.get("x-open-compute-entrypoint") || undefined;
    const envelope = assertEnvelope(request, validation, entrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(
      env,
      envelope,
      validation,
      Boolean(entrypoint),
      internalToken,
    );
    const runtimeKey = validation
      ? `${envelope.runtimeKey}/validation`
      : envelope.runtimeKey;
    const versionId = envelope.loaderKey.split("/")[2]!;
    const tenant = validation ? undefined : tenantRequest(request);
    if (
      !validation &&
      !entrypoint &&
      tenant &&
      routeDefaultHttp(snapshot, tenant) === "asset"
    ) {
      const response = await ctx.exports
        .AssetTransport({
          props: Object.freeze({
            versionId,
            descriptorSha256: snapshot.workerCodeSha256,
          }),
        })
        .fetch(tenant);
      const headers = new Headers(response.headers);
      const representationLength =
        headers.get("x-open-compute-asset-representation-length") ??
        headers.get("content-length");
      for (const name of INTERNAL_HEADERS) headers.delete(name);
      if (representationLength) {
        headers.set(
          "x-open-compute-asset-representation-length",
          representationLength,
        );
      }
      headers.set("x-open-compute-request-id", requestId);
      headers.set("x-open-compute-loader-outcome", "asset");
      const forwarded = new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
      if (representationLength)
        forwarded.headers.set("content-length", representationLength);
      return forwarded;
    }
    if (snapshot.contentKind !== "worker")
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    let cold = false;
    const stub = env.LOADER.get(runtimeKey, async () => {
      cold = true;
      const code = await assembleOnce(runtimeKey, async () => {
        const built = modulesFor(snapshot, validation, entrypoint);
        return {
          ...snapshotWorkerCode(snapshot),
          mainModule: built.mainModule,
          modules: built.modules,
          env: validation
            ? {}
            : tenantEnv(
                snapshot,
                ctx,
                env.WORKER_LOADER_FACTORY,
                versionId,
                doPolicy(env),
                false,
                entrypoint ?? "default",
              ),
          globalOutbound: tenantGlobalOutbound(env, validation),
        };
      });
      return code;
    });
    const target = validation
      ? stub.getEntrypoint()
      : observedEntrypoint(
          stub,
          env.WORKER_LOADER_FACTORY,
          ctx,
          snapshot.observability,
          entrypoint,
        );
    executionStarted = !validation;
    const response = await target.fetch(
      validation ? "https://validation.invalid/" : tenant!,
    );
    if (validation) {
      const body = await response.text();
      if (response.status !== 200 || body !== "open-compute-validation-v1") {
        throw new Error("validation nonce mismatch");
      }
      return new Response(null, { status: 204 });
    }
    const headers = new Headers(response.headers);
    const representationLength = headers.get(
      "x-open-compute-asset-representation-length",
    );
    const serviceWebSocketHandoffs = serviceWebSocketHandoffHandles(response);
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    if (representationLength) {
      headers.set(
        "x-open-compute-asset-representation-length",
        representationLength,
      );
    }
    headers.set("x-open-compute-request-id", requestId);
    headers.set("x-open-compute-loader-outcome", cold ? "cold" : "warm");
    if (executionStarted) headers.set("x-open-compute-execution-started", "1");
    if (serviceWebSocketHandoffs.length > 0) {
      headers.set(
        SERVICE_WEBSOCKET_HANDOFF_HEADER,
        serviceWebSocketHandoffs.join(","),
      );
    }
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
      webSocket: response.webSocket,
    });
  } catch (error) {
    const stable = stableCode(error);
    if (stable) {
      const status =
        stable === "VERSION_NOT_READY"
          ? 409
          : stable === "ARTIFACT_UNAVAILABLE"
            ? 503
            : stable === "BUNDLE_RUNTIME_INVALID"
              ? 422
              : 500;
      const response = stableError(stable, status, requestId);
      if (executionStarted)
        response.headers.set("x-open-compute-execution-started", "1");
      return response;
    }
    const [code, status] = classify(error);
    const response = stableError(code, status, requestId);
    if (executionStarted)
      response.headers.set("x-open-compute-execution-started", "1");
    return response;
  }
}

function customEventMessageBody(message: Record<string, unknown>) {
  if (
    !message ||
    typeof message !== "object" ||
    typeof message.bodyBase64 !== "string"
  ) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  const raw = bytes(message.bodyBase64);
  if (raw.byteLength > MAX_QUEUE_BODY_BYTES) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  let body: unknown;
  switch (message.contentType) {
    case "json":
      body = JSON.parse(
        new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(raw),
      );
      break;
    case "text":
      body = new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(
        raw,
      );
      break;
    case "bytes":
      body = raw;
      break;
    case "v8":
      body = decodeDurableValue(raw, "queue-v8");
      break;
    default:
      throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  return { body, byteLength: raw.byteLength };
}

function queueBatchMetadata(input: unknown): {
  metrics: {
    backlogCount: number;
    backlogBytes: number;
    oldestMessageTimestamp?: Date;
  };
} {
  if (input === undefined) {
    return { metrics: { backlogCount: 0, backlogBytes: 0 } };
  }
  if (!isRecord(input) || !isRecord(input.metrics)) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  const metrics = input.metrics;
  if (
    typeof metrics.backlogCount !== "number" ||
    !Number.isSafeInteger(metrics.backlogCount) ||
    metrics.backlogCount < 0 ||
    typeof metrics.backlogBytes !== "number" ||
    !Number.isSafeInteger(metrics.backlogBytes) ||
    metrics.backlogBytes < 0
  ) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  const oldest = metrics.oldestMessageTimestampMs;
  const output: {
    backlogCount: number;
    backlogBytes: number;
    oldestMessageTimestamp?: Date;
  } = {
    backlogCount: metrics.backlogCount,
    backlogBytes: metrics.backlogBytes,
  };
  if (oldest !== undefined && oldest !== null && oldest !== 0) {
    if (typeof oldest !== "number" || !Number.isSafeInteger(oldest)) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    output.oldestMessageTimestamp = new Date(oldest);
  }
  return { metrics: output };
}

async function customEventTarget(
  request: Request,
  env: LoaderEnv,
  ctx: ExecutionContext,
) {
  const entrypoint =
    request.headers.get("x-open-compute-entrypoint") || undefined;
  const envelope = assertEnvelope(request, false, entrypoint);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(
    env,
    envelope,
    false,
    Boolean(entrypoint),
    internalToken,
  );
  const runtimeKey = envelope.runtimeKey;
  let cold = false;
  const stub = env.LOADER.get(runtimeKey, async () => {
    cold = true;
    return assembleOnce(runtimeKey, async () => {
      const built = modulesFor(snapshot, false, entrypoint);
      const versionId = envelope.loaderKey.split("/")[2]!;
      const code = {
        ...snapshotWorkerCode(snapshot),
        mainModule: built.mainModule,
        modules: built.modules,
        env: tenantEnv(
          snapshot,
          ctx,
          env.WORKER_LOADER_FACTORY,
          versionId,
          doPolicy(env),
          false,
          entrypoint ?? "default",
        ),
        globalOutbound: tenantGlobalOutbound(env, false),
      };
      return code;
    });
  });
  return {
    target: observedEntrypoint(
      stub,
      env.WORKER_LOADER_FACTORY,
      ctx,
      snapshot.observability,
      entrypoint,
    ),
    snapshot,
    loaderOutcome: () => (cold ? "cold" : "warm"),
  };
}

export async function handleQueue(
  request: Request,
  env: LoaderEnv,
  ctx: ExecutionContext,
) {
  try {
    const payload: unknown = await request.json();
    if (
      !isRecord(payload) ||
      typeof payload.queueName !== "string" ||
      payload.queueName.length < 1 ||
      payload.queueName.length > 128 ||
      !Array.isArray(payload.messages) ||
      payload.messages.length < 1 ||
      payload.messages.length > MAX_QUEUE_MESSAGES
    ) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    let totalBytes = 0;
    const messages = payload.messages.map((message: unknown) => {
      if (
        !isRecord(message) ||
        typeof message.id !== "string" ||
        typeof message.timestampMs !== "number" ||
        !Number.isSafeInteger(message.timestampMs) ||
        message.timestampMs < 0 ||
        typeof message.attempts !== "number" ||
        !Number.isSafeInteger(message.attempts) ||
        message.attempts < 1 ||
        message.attempts > 101
      ) {
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
    const result = await loaded.target.queue(
      payload.queueName,
      messages,
      queueBatchMetadata(payload.metadata),
    );
    const response = Response.json(result);
    response.headers.set(
      "x-open-compute-loader-outcome",
      loaded.loaderOutcome(),
    );
    return response;
  } catch (error) {
    const stable = stableCode(error);
    return stableError(
      stable || "QUEUE_CUSTOM_EVENT_UNSUPPORTED",
      stable ? 422 : 500,
      null,
    );
  }
}

export async function handleScheduled(
  request: Request,
  env: LoaderEnv,
  ctx: ExecutionContext,
) {
  try {
    const payload: unknown = await request.json();
    if (!isRecord(payload)) throw bindingError("CRON_EXPRESSION_INVALID");
    const workflowBindings: unknown = payload.workflowBindings;
    if (
      typeof payload.scheduledTimeMs !== "number" ||
      !Number.isSafeInteger(payload.scheduledTimeMs) ||
      payload.scheduledTimeMs < 0 ||
      payload.scheduledTimeMs % 60_000 !== 0 ||
      typeof payload.cron !== "string" ||
      payload.cron.length < 1 ||
      payload.cron.length > 256 ||
      typeof payload.scheduledHandler !== "boolean" ||
      !Array.isArray(workflowBindings) ||
      workflowBindings.length > 100 ||
      (!payload.scheduledHandler && workflowBindings.length === 0) ||
      !workflowBindings.every(
        (value, index) =>
          typeof value === "string" &&
          SCHEDULED_WORKFLOW_BINDING.test(value) &&
          !value.startsWith("OPEN_COMPUTE_") &&
          !value.startsWith("__") &&
          (index === 0 || workflowBindings[index - 1] < value),
      )
    ) {
      throw bindingError("CRON_EXPRESSION_INVALID");
    }
    const loaded = await customEventTarget(request, env, ctx);
    const target = loaded.snapshot.scheduledTargets.find(
      (value) => value.cron === payload.cron,
    );
    if (
      !target ||
      target.scheduledHandler !== payload.scheduledHandler ||
      target.workflowBindings.length !== workflowBindings.length ||
      target.workflowBindings.some(
        (value, index) => value !== workflowBindings[index],
      )
    ) {
      throw bindingError("CRON_ACTIVATION_STALE");
    }
    const scheduledEvent = {
      scheduledTime: new Date(payload.scheduledTimeMs),
      cron: payload.cron,
    };
    const result = await loaded.target.scheduled(scheduledEvent);
    const response = Response.json(result);
    response.headers.set(
      "x-open-compute-loader-outcome",
      loaded.loaderOutcome(),
    );
    return response;
  } catch (error) {
    const stable = stableCode(error);
    return stableError(
      stable || "CRON_CUSTOM_EVENT_UNSUPPORTED",
      stable ? 422 : 500,
      null,
    );
  }
}

function moduleExportsDurableObjectClass(
  modules: readonly RuntimeModule[],
  className: string,
): boolean {
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

export async function validateDurableObjectClass(
  request: Request,
  env: LoaderEnv,
) {
  const className = request.headers.get("x-open-compute-entrypoint") || "";
  const envelope = assertEnvelope(request, true, className);
  const internalToken = request.headers.get(TOKEN_HEADER) || "";
  const snapshot = await resolveSnapshot(
    env,
    envelope,
    true,
    false,
    internalToken,
  );
  if (!moduleExportsDurableObjectClass(snapshot.modules, className)) {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
  const built = modulesFor(snapshot, false, className, true);
  const code = {
    ...snapshotWorkerCode(snapshot),
    mainModule: built.mainModule,
    modules: built.modules,
    env: {},
    globalOutbound: null,
  };
  try {
    const loaded = env.LOADER.get(
      `validate-do/${envelope.runtimeKey}`,
      () => code,
    );
    loaded.getDurableObjectClass(className);
    return new Response(null, { status: 204 });
  } catch {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
}
