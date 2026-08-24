import { WorkerEntrypoint } from "cloudflare:workers";

const SOURCE_PATH = "/internal/runtime/v1/deployments/resolve";
const TOKEN_HEADER = "x-open-compute-internal-token";
let startupGeneration;
const assembling = new Map();
const seenHashes = new Map();
const INTERNAL_HEADERS = [
  TOKEN_HEADER,
  "x-open-compute-account-id",
  "x-open-compute-worker-id",
  "x-open-compute-deployment-id",
  "x-open-compute-loader-key",
  "x-open-compute-worker-code-sha256",
  "x-open-compute-entrypoint",
  "x-open-compute-original-method",
  "x-open-compute-original-url",
  "x-open-compute-route-generation",
  "x-open-compute-request-id",
  "forwarded",
  "x-forwarded-for",
  "x-forwarded-host",
  "x-forwarded-proto",
];

const PROFILE = Object.freeze({ cpuMs: 50, subRequests: 16 });

function currentStartupGeneration() {
  if (!startupGeneration) startupGeneration = crypto.randomUUID();
  return startupGeneration;
}

function stableError(code, status, requestId) {
  return Response.json({
    ok: false,
    error: { code, message: "worker request failed", requestId: requestId || null },
  }, { status });
}

function classify(error) {
  const message = String(error && error.message ? error.message : error);
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

function bytes(base64) {
  const binary = atob(base64);
  const value = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) value[i] = binary.charCodeAt(i);
  return value;
}

function moduleValue(module) {
  const raw = bytes(module.bytesBase64);
  switch (module.type) {
    case "esModule":
      return { js: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "commonJsModule":
      return { cjs: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "text":
      return { text: new TextDecoder("utf-8", { fatal: true }).decode(raw) };
    case "json":
      return { json: JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(raw)) };
    case "data":
      return { data: raw };
    case "wasm":
      return { wasm: raw };
    default:
      throw new Error("unsupported module representation");
  }
}

function modulesFor(snapshot, validation, validationEntrypoint) {
  const modules = {};
  for (const module of snapshot.modules) modules[module.name] = moduleValue(module);
  if (validation) {
    const wrapper = "__open_compute_validation__.js";
    const exportName = validationEntrypoint || "default";
    modules[wrapper] = { js: `import * as tenant from ${JSON.stringify(snapshot.mainModule)};\nif (!(${JSON.stringify(exportName)} in tenant)) throw new Error(\"missing entrypoint\");\nexport default { fetch() { return new Response(\"open-compute-validation-v1\"); } };` };
    return { modules, mainModule: wrapper };
  }
  return { modules, mainModule: snapshot.mainModule };
}

function assertEnvelope(request, validation, validationEntrypoint) {
  const loaderKey = request.headers.get("x-open-compute-loader-key") || "";
  const expected = request.headers.get("x-open-compute-worker-code-sha256") || "";
  const parts = loaderKey.split("/");
  if (parts.length !== 3 || parts.some((part) => !/^[0-9a-f]{8}-[0-9a-f-]{27}$/.test(part))) {
    throw new Error("invalid loader key");
  }
  if (!/^[0-9a-f]{64}$/.test(expected)) throw new Error("invalid descriptor hash");
  if (validationEntrypoint && !/^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/.test(validationEntrypoint)) {
    throw new Error("invalid entrypoint");
  }
  return {
    loaderKey,
    expected,
    runtimeKey: `${validation ? "validate" : "runtime"}/${loaderKey}${validation ? `/${expected}/${validationEntrypoint || "default"}` : ""}`,
  };
}

async function resolveSnapshot(env, envelope, validation, probe, internalToken) {
  const response = await env.RUNTIME_SOURCE.fetch(`http://runtime-source${SOURCE_PATH}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [TOKEN_HEADER]: internalToken,
    },
    body: JSON.stringify({
      startupGeneration: currentStartupGeneration(),
      key: envelope.loaderKey,
      expectedWorkerCodeSha256: envelope.expected,
      scope: validation ? (probe ? "probe" : "validation") : "runtime",
    }),
  });
  if (!response.ok) {
    const code = response.headers.get("x-open-compute-error-code") || "RUNTIME_INTERNAL";
    const error = new Error(code);
    error.stableCode = code;
    throw error;
  }
  const snapshot = await response.json();
  if (snapshot.loaderKey !== envelope.loaderKey || snapshot.workerCodeSha256 !== envelope.expected) {
    const error = new Error("DEPLOYMENT_INVARIANT_VIOLATION");
    error.stableCode = "DEPLOYMENT_INVARIANT_VIOLATION";
    throw error;
  }
  return snapshot;
}

function assembleOnce(key, build) {
  const current = assembling.get(key);
  if (current) return current;
  const pending = build().finally(() => {
    if (assembling.get(key) === pending) assembling.delete(key);
  });
  assembling.set(key, pending);
  return pending;
}

function tenantRequest(request) {
  const headers = new Headers(request.headers);
  const method = request.headers.get("x-open-compute-original-method") || "GET";
  const url = request.headers.get("x-open-compute-original-url") || "https://worker.invalid/";
  for (const name of INTERNAL_HEADERS) headers.delete(name);
  const init = { method, headers, body: request.body, redirect: "manual" };
  if (method === "GET" || method === "HEAD") delete init.body;
  return new Request(url, init);
}

export class OutboundGateway extends WorkerEntrypoint {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.protocol !== "http:" && url.protocol !== "https:") {
      throw new TypeError("OUTBOUND_DENIED");
    }
    return fetch(new Request(request, { redirect: "follow" }));
  }
}

async function handle(request, env, ctx, validation) {
  const requestId = request.headers.get("x-open-compute-request-id") || crypto.randomUUID();
  try {
    const validationEntrypoint = validation ? (request.headers.get("x-open-compute-entrypoint") || undefined) : undefined;
    const envelope = assertEnvelope(request, validation, validationEntrypoint);
    const internalToken = request.headers.get(TOKEN_HEADER) || "";
    // Resolve and verify on every path, including a warm WorkerLoader key.
    const snapshot = await resolveSnapshot(env, envelope, validation, Boolean(validationEntrypoint), internalToken);
    const prior = seenHashes.get(envelope.runtimeKey);
    if (prior && prior !== snapshot.workerCodeSha256) {
      const error = new Error("DEPLOYMENT_INVARIANT_VIOLATION");
      error.stableCode = "DEPLOYMENT_INVARIANT_VIOLATION";
      throw error;
    }
    seenHashes.set(envelope.runtimeKey, snapshot.workerCodeSha256);
    const code = await assembleOnce(envelope.runtimeKey, async () => {
      const built = modulesFor(snapshot, validation, validationEntrypoint);
      return {
        compatibilityDate: snapshot.compatibilityDate,
        compatibilityFlags: snapshot.compatibilityFlags,
        mainModule: built.mainModule,
        modules: built.modules,
        env: validation ? {} : snapshot.env,
        globalOutbound: validation ? null : ctx.exports.OutboundGateway({
          props: { deploymentId: envelope.loaderKey.split("/")[2], policyVersion: 1 },
        }),
        limits: PROFILE,
      };
    });
    let cold = false;
    const stub = env.LOADER.get(envelope.runtimeKey, async () => {
      cold = true;
      return code;
    });
    const entrypoint = validation ? undefined : (request.headers.get("x-open-compute-entrypoint") || undefined);
    const target = stub.getEntrypoint(entrypoint, { limits: PROFILE });
    const response = await target.fetch(validation ? "https://validation.invalid/" : tenantRequest(request));
    if (validation) {
      const body = await response.text();
      if (response.status !== 200 || body !== "open-compute-validation-v1") {
        throw new Error("validation nonce mismatch");
      }
      return new Response(null, { status: 204 });
    }
    const headers = new Headers(response.headers);
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    headers.set("x-open-compute-request-id", requestId);
    headers.set("x-open-compute-loader-outcome", cold ? "cold" : "warm");
    return new Response(response.body, { status: response.status, statusText: response.statusText, headers });
  } catch (error) {
    const stable = error && error.stableCode;
    if (stable) {
      const status = stable === "DEPLOYMENT_NOT_READY" ? 409
        : stable === "ARTIFACT_UNAVAILABLE" ? 503
        : stable === "BUNDLE_RUNTIME_INVALID" ? 422
        : 500;
      return stableError(stable, status, requestId);
    }
    const [code, status] = classify(error);
    return stableError(code, status, requestId);
  }
}

export default {
  async fetch(request, env, ctx) {
    const path = new URL(request.url).pathname;
    if (request.method === "POST" && path === "/internal/dispatch") return handle(request, env, ctx, false);
    if (request.method === "POST" && path === "/internal/validate") return handle(request, env, ctx, true);
    return new Response(null, { status: 404 });
  },
};
