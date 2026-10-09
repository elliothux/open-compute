import { routeDefaultHttp } from "../assets/router.js";
import { observedEntrypoint } from "../observability/collector.js";
import { queueMetrics } from "../queues/metrics.js";
import type { QueueWireCodec } from "../queues/native-adapter.js";
import {
  SERVICE_WEBSOCKET_HANDOFF_HEADER,
  serviceWebSocketHandoffHandles,
} from "../services/facade.js";
import { tenantEnv, validationEnv } from "./bindings.js";
import { assertEnvelope } from "./envelope.js";
import { modulesFor } from "./modules.js";
import type { LoaderEnv } from "./protocol.js";
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
const MAX_QUEUE_BODY_BYTES = 128000;
const MAX_QUEUE_BATCH_BYTES = 256000;
const SCHEDULED_WORKFLOW_BINDING = /^[A-Za-z_][A-Za-z0-9_]{0,63}$/;
const SERVICE_ERRORS = [
  ["SERVICE_BINDING_DENIED", 403],
  ["SERVICE_TARGET_NOT_READY", 503],
  ["SERVICE_UNAVAILABLE", 503],
  ["SERVICE_ENTRYPOINT_NOT_FOUND", 404],
  ["SERVICE_LIMIT_EXCEEDED", 429],
  ["SERVICE_TIMEOUT", 504],
] as const;

const DO_ERRORS = [
  ["DO_DISPATCH_TIMEOUT", 504],
  ["DO_STORAGE_LIMIT", 429],
  ["DO_STORAGE_UNAVAILABLE", 503],
  ["DO_RUNTIME_EXCEPTION", 500],
  ["DO_RPC_UNSUPPORTED", 400],
  ["DO_INTERNAL_PROTOCOL_ERROR", 500],
  ["DO_OBJECT_DELETING", 409],
  ["DO_ID_INVALID", 400],
  ["DO_CLASS_NOT_FOUND", 422],
  ["DO_NAMESPACE_NOT_FOUND", 404],
  ["DO_VERSION_STALE", 409],
  ["DO_OUTPUT_GATE_UNPUBLISHABLE", 500],
] as const;

const WORKFLOW_ERRORS = [
  ["WORKFLOW_RUNTIME_UNAVAILABLE", 503],
  ["WORKFLOW_VERSION_NOT_READY", 503],
  ["WORKFLOW_BINDING_STALE", 409],
  ["WORKFLOW_METHOD_UNSUPPORTED", 400],
  ["WORKFLOW_INSTANCE_ID_INVALID", 400],
  ["WORKFLOW_PAYLOAD_TOO_LARGE", 413],
  ["WORKFLOW_INVARIANT_VIOLATION", 500],
] as const;

const STABLE_ERROR_STATUS = new Map<string, number>([
  ...SERVICE_ERRORS,
  ...DO_ERRORS,
  ...WORKFLOW_ERRORS,
  ["VERSION_NOT_READY", 409],
  ["ARTIFACT_UNAVAILABLE", 503],
  ["BUNDLE_RUNTIME_INVALID", 422],
]);

function stableError(
  code: string,
  status: number,
  requestId?: string | null,
  cloudflare?: { code: number; outcome: string },
): Response {
  const response = Response.json(
    {
      ok: false,
      error: {
        code,
        message: "worker request failed",
        requestId: requestId || null,
        ...(cloudflare === undefined
          ? {}
          : {
              cloudflareCode: cloudflare.code,
              outcome: cloudflare.outcome,
            }),
      },
    },
    { status },
  );
  if (cloudflare?.code === 1101 || cloudflare?.code === 1102) {
    response.headers.set("cf-error-type", String(cloudflare.code));
  }
  return response;
}

function classify(
  error: unknown,
  validation: boolean,
): [string, number, { code: number; outcome: string }?] {
  const message = String(error instanceof Error ? error.message : error);
  for (const [code, status] of SERVICE_ERRORS) {
    if (message.includes(code)) return [code, status];
  }
  for (const [code, status] of DO_ERRORS) {
    if (message.includes(code)) return [code, status];
  }
  for (const [code, status] of WORKFLOW_ERRORS) {
    if (message.includes(code)) return [code, status];
  }
  if (/entrypoint|no such entrypoint|was not found/i.test(message)) {
    return ["ENTRYPOINT_NOT_FOUND", 404];
  }
  if (/cpu time limit/i.test(message)) {
    return [
      "RESOURCE_LIMIT_EXCEEDED",
      500,
      { code: 1102, outcome: "exceededCpu" },
    ];
  }
  if (/memory limit/i.test(message)) {
    return [
      "RESOURCE_LIMIT_EXCEEDED",
      500,
      { code: 1102, outcome: "exceededMemory" },
    ];
  }
  if (/limit|subrequest/i.test(message)) {
    return [
      "RESOURCE_LIMIT_EXCEEDED",
      500,
      { code: 1101, outcome: "exception" },
    ];
  }
  if (
    validation &&
    /compatibility (?:date|flag|flags)|syntax|parse|unexpected|module|wasm|initializ|startup/i.test(
      message,
    )
  ) {
    return ["BUNDLE_RUNTIME_INVALID", 422];
  }
  return ["RUNTIME_INTERNAL", 500];
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
  probe = false,
) {
  const requestId =
    request.headers.get("x-open-compute-request-id") || crypto.randomUUID();
  let executionStarted = false;
  try {
    const entrypoint =
      request.headers.get("x-open-compute-entrypoint") || undefined;
    const envelope = assertEnvelope(request, validation, entrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    const scope = validation
      ? probe || entrypoint
        ? "probe"
        : "validation"
      : "runtime";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(env, envelope, scope, internalToken);
    const runtimeKey = validation
      ? `${envelope.runtimeKey}/validation`
      : envelope.runtimeKey;
    const versionId = envelope.loaderKey.split("/")[2]!;
    const tenant = validation ? undefined : tenantRequest(request);
    if (validation && probe && snapshot.contentKind === "assets_only") {
      return new Response(null, { status: 204 });
    }
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
          ...(await snapshotWorkerCode(env, snapshot, scope, internalToken)),
          mainModule: built.mainModule,
          modules: built.modules,
          ...(validation
            ? validationEnv(snapshot, built.policy, env.WORKER_LOADER_FACTORY)
            : tenantEnv(
                snapshot,
                built.policy,
                ctx,
                env.WORKER_LOADER_FACTORY,
                versionId,
                doPolicy(env),
                false,
                entrypoint ?? "default",
              )),
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
    if (
      !validation &&
      response.headers.get("x-open-compute-resource-limit") === "subrequests"
    ) {
      const failure = stableError("RESOURCE_LIMIT_EXCEEDED", 500, requestId, {
        code: 1101,
        outcome: "exception",
      });
      failure.headers.set("x-open-compute-execution-started", "1");
      return failure;
    }
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
      const status = STABLE_ERROR_STATUS.get(stable) ?? 500;
      const response = stableError(stable, status, requestId);
      if (executionStarted)
        response.headers.set("x-open-compute-execution-started", "1");
      return response;
    }
    const [code, status, cloudflare] = classify(error, validation);
    const response =
      validation && code === "RESOURCE_LIMIT_EXCEEDED"
        ? stableError("BUNDLE_RUNTIME_INVALID", 422, requestId, {
            code: 10021,
            outcome: cloudflare?.outcome ?? "exception",
          })
        : stableError(code, status, requestId, cloudflare);
    if (executionStarted)
      response.headers.set("x-open-compute-execution-started", "1");
    return response;
  }
}

function customEventMessageBody(
  message: Record<string, unknown>,
  codec: QueueWireCodec,
) {
  if (
    !message ||
    typeof message !== "object" ||
    typeof message.bodyBase64 !== "string" ||
    message.bodyBase64.length > Math.ceil(MAX_QUEUE_BODY_BYTES / 3) * 4
  ) {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
  try {
    const binary = atob(message.bodyBase64);
    if (btoa(binary) !== message.bodyBase64) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    const raw = Uint8Array.from(binary, (char) => char.charCodeAt(0));
    if (raw.byteLength > MAX_QUEUE_BODY_BYTES) {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    let body: unknown;
    switch (message.contentType) {
      case "json":
        body = JSON.parse(
          new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(
            raw,
          ),
        );
        break;
      case "text":
        body = new TextDecoder("utf-8", {
          fatal: true,
          ignoreBOM: false,
        }).decode(raw);
        break;
      case "bytes":
        body = raw;
        break;
      case "v8":
        body = codec.decodeV8(raw);
        if (body === undefined) throw bindingError("QUEUE_DISPOSITION_INVALID");
        break;
      default:
        throw bindingError("QUEUE_DISPOSITION_INVALID");
    }
    return { body, byteLength: raw.byteLength };
  } catch {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
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
  try {
    return { metrics: queueMetrics(input.metrics) };
  } catch {
    throw bindingError("QUEUE_DISPOSITION_INVALID");
  }
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
    "runtime",
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
        ...(await snapshotWorkerCode(env, snapshot, "runtime", internalToken)),
        mainModule: built.mainModule,
        modules: built.modules,
        ...tenantEnv(
          snapshot,
          built.policy,
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
    const payload: unknown = await request.json().catch(() => {
      throw bindingError("QUEUE_DISPOSITION_INVALID");
    });
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
      const decoded = customEventMessageBody(message, env.QUEUE_WIRE_CODEC);
      totalBytes += decoded.byteLength;
      if (totalBytes > MAX_QUEUE_BATCH_BYTES) {
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      }
      const timestamp = new Date(message.timestampMs);
      if (!Number.isFinite(timestamp.getTime()))
        throw bindingError("QUEUE_DISPOSITION_INVALID");
      return {
        id: message.id,
        timestamp,
        attempts: message.attempts,
        body: decoded.body,
      };
    });
    const metadata = queueBatchMetadata(payload.metadata);
    const loaded = await customEventTarget(request, env, ctx);
    const result = await loaded.target.queue(
      payload.queueName,
      messages,
      metadata,
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
    const payload: unknown = await request.json().catch(() => {
      throw bindingError("CRON_EXPRESSION_INVALID");
    });
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
    const scheduledTime = new Date(payload.scheduledTimeMs);
    if (!Number.isFinite(scheduledTime.getTime()))
      throw bindingError("CRON_EXPRESSION_INVALID");
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
      scheduledTime,
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
    "validation",
    internalToken,
  );
  try {
    const loaded = env.LOADER.get(
      `validate-do/${envelope.runtimeKey}`,
      async () => {
        const built = modulesFor(snapshot, true, className, true);
        return {
          ...(await snapshotWorkerCode(
            env,
            snapshot,
            "validation",
            internalToken,
          )),
          mainModule: built.mainModule,
          modules: built.modules,
          ...validationEnv(snapshot, built.policy, env.WORKER_LOADER_FACTORY),
          globalOutbound: null,
        };
      },
    );
    // WorkerLoader class handles are lazy; a host probe must complete loading.
    const response = await loaded
      .getEntrypoint()
      .fetch("https://validation.invalid/");
    if (
      response.status !== 200 ||
      (await response.text()) !== "open-compute-validation-v1"
    )
      throw new Error("validation nonce mismatch");
    return new Response(null, { status: 204 });
  } catch {
    return stableError("DO_CLASS_NOT_FOUND", 422, null);
  }
}
