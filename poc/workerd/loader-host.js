import { WorkerEntrypoint } from "cloudflare:workers";
import { assembleWorkerCode, fingerprintWorkerCode, getSeenHash } from "./code.js";
import { tenantError, classifyThrown } from "./errors.js";
import { logEvent, sha256hex } from "./log.js";
import { getDeployment, loaderKey } from "./registry.js";

const callbackCounts = new Map();
const lastOutcome = new Map();
const routes = new Map();
const assembling = new Map();
let faults = {};

const UNIMPLEMENTED_KINDS = new Set(["scheduled", "queue", "workflow"]);
const IDENTITY_HEADERS = new Set(["x-account-id", "x-deployment-id", "x-worker-id"]);

function bumpCallback(key) {
  callbackCounts.set(key, (callbackCounts.get(key) ?? 0) + 1);
}

function json(data, status = 200) {
  return Response.json(data, { status });
}

function stripIdentityHeaders(headers) {
  const inner = new Headers(headers || {});
  for (const name of IDENTITY_HEADERS) {
    inner.delete(name);
  }
  inner.delete("x-g0-request-id");
  return inner;
}

function frozenBindingProps(spec) {
  if (!spec || spec.kind !== "binding") return null;
  if (typeof spec.resourceId !== "string" || !spec.resourceId) {
    throw new Error("BINDING_HOST_UNAVAILABLE");
  }
  return Object.freeze({
    accountId: spec.accountId,
    resourceId: spec.resourceId,
    deploymentId: spec.deploymentId,
  });
}

function scopedKvStub(ctx, spec) {
  const props = frozenBindingProps(spec);
  if (!props) return null;
  const factory = ctx.exports && ctx.exports.FixtureKV;
  if (typeof factory === "function") {
    return factory({ props });
  }
  if (factory && typeof factory.getEntrypoint === "function") {
    return factory.getEntrypoint(undefined, { props });
  }
  throw new Error("BINDING_HOST_UNAVAILABLE");
}

async function buildCode(env, ctx, key, options = {}) {
  if (faults.F1) {
    throw new Error("FAULT_INJECTED:F1");
  }
  const spec = options.specOverride || getDeployment(key);
  const kvStub = scopedKvStub(ctx, spec);
  const assembled = await assembleWorkerCode(env, key, {
    kvStub,
    specOverride: spec,
    extraEnv: {
      G0_IDENTITY: spec
        ? {
            accountId: spec.accountId,
            workerId: spec.workerId,
            deploymentId: spec.deploymentId,
          }
        : null,
    },
    dryRun: options.dryRun,
  });
  if (faults.F2) {
    throw new Error("FAULT_INJECTED:F2");
  }
  return assembled;
}

function assembleOnce(env, ctx, key) {
  const existing = assembling.get(key);
  if (existing) return existing;
  const pending = buildCode(env, ctx, key).finally(() => {
    if (assembling.get(key) === pending) assembling.delete(key);
  });
  assembling.set(key, pending);
  return pending;
}

function sanitizedBindingError(code) {
  const err = new Error(code);
  err.name = "Error";
  err.errorCode = code;
  err.stack = `Error: ${code}`;
  return err;
}

export class FixtureKV extends WorkerEntrypoint {
  #scope() {
    const props = this.ctx.props;
    if (!props || typeof props.resourceId !== "string" || !props.resourceId) {
      throw sanitizedBindingError("BINDING_INTERNAL");
    }
    return {
      accountId: props.accountId,
      resourceId: props.resourceId,
      deploymentId: props.deploymentId,
    };
  }

  async get(key, _claim) {
    void _claim;
    const scope = this.#scope();
    if (typeof key !== "string") {
      throw sanitizedBindingError("BINDING_TYPE");
    }
    try {
      const value = await this.env.BINDING_BACKEND.get(scope.resourceId, key);
      logEvent({
        deploymentId: scope.deploymentId ?? null,
        bindingType: "FixtureKV",
        resourceIdHash: await sha256hex(scope.resourceId),
        outcome: "ok",
        extra: { op: "get" },
      });
      return value;
    } catch (err) {
      if (err && err.errorCode === "BINDING_TYPE") {
        throw sanitizedBindingError("BINDING_TYPE");
      }
      logEvent({
        deploymentId: scope.deploymentId ?? null,
        bindingType: "FixtureKV",
        resourceIdHash: await sha256hex(scope.resourceId),
        outcome: "error",
        errorCode: "BINDING_INTERNAL",
        extra: { op: "get" },
      });
      throw sanitizedBindingError("BINDING_INTERNAL");
    }
  }

  async put(key, value, _claim) {
    void _claim;
    const scope = this.#scope();
    if (typeof key !== "string" || typeof value !== "string") {
      throw sanitizedBindingError("BINDING_TYPE");
    }
    try {
      await this.env.BINDING_BACKEND.put(scope.resourceId, key, value);
      logEvent({
        deploymentId: scope.deploymentId ?? null,
        bindingType: "FixtureKV",
        resourceIdHash: await sha256hex(scope.resourceId),
        outcome: "ok",
        extra: { op: "put" },
      });
    } catch (err) {
      if (err && (err.errorCode === "BINDING_TYPE" || err.name === "TypeError")) {
        throw sanitizedBindingError("BINDING_TYPE");
      }
      logEvent({
        deploymentId: scope.deploymentId ?? null,
        bindingType: "FixtureKV",
        resourceIdHash: await sha256hex(scope.resourceId),
        outcome: "error",
        errorCode: "BINDING_INTERNAL",
        extra: { op: "put" },
      });
      throw sanitizedBindingError("BINDING_INTERNAL");
    }
  }

  async fetch() {
    throw sanitizedBindingError("BINDING_DENIED");
  }
}

function innerRequest(body, request, requestId) {
  const innerUrl = body.url || "https://g0.invalid/";
  const innerHeaders = stripIdentityHeaders(body.headers || {});
  innerHeaders.set("x-g0-request-id", requestId);
  const method = body.method || (body.body != null ? "POST" : request.method || "GET");
  const init = { method, headers: innerHeaders };
  if (body.body != null && method !== "GET" && method !== "HEAD") {
    init.body = typeof body.body === "string" ? body.body : JSON.stringify(body.body);
  }
  try {
    return new Request(innerUrl, { ...init, signal: request.signal });
  } catch {
    return new Request(innerUrl, init);
  }
}

async function dispatch(request, env, ctx, body) {
  const started = Date.now();
  const requestId = body.requestId || crypto.randomUUID();
  const kind = body.kind ?? "fetch";
  const accountId = body.accountId;
  const workerId = body.workerId;
  const deploymentId = body.deploymentId;
  const entrypoint = body.entrypoint ?? null;
  const key = loaderKey(accountId, workerId, deploymentId);
  const loaderKeyHash = await sha256hex(key);
  const spec = getDeployment(key);
  const bindingType = spec?.kind === "binding" ? "FixtureKV" : null;
  const resourceIdHash = spec?.resourceId ? await sha256hex(spec.resourceId) : null;

  const fail = (errorCode, status, loaderOutcome = "error") => {
    logEvent({
      requestId,
      deploymentId: deploymentId ?? null,
      loaderKeyHash,
      loaderOutcome,
      dispatchKind: kind,
      entrypoint,
      bindingType,
      resourceIdHash,
      durationMs: Date.now() - started,
      outcome: "error",
      errorCode,
    });
    return tenantError(errorCode, requestId, deploymentId, status);
  };

  if (kind !== "fetch") {
    if (UNIMPLEMENTED_KINDS.has(kind)) {
      return fail("DISPATCH_KIND_UNSUPPORTED", 400);
    }
    return fail("DISPATCH_KIND_UNKNOWN", 400);
  }

  if (!spec) {
    return fail("DEPLOYMENT_NOT_FOUND", 404);
  }

  let cold = false;
  try {
    // Hash is checked before LOADER.get(): workerd will not re-invoke the
    // callback for a warm key, so remapping must be rejected by the host.
    const assembled = await assembleOnce(env, ctx, key);
    const worker = env.LOADER.get(key, async () => {
      cold = true;
      bumpCallback(key);
      return assembled.code;
    });

    const target = entrypoint ? worker.getEntrypoint(entrypoint) : worker.getEntrypoint();
    const inner = innerRequest(body, request, requestId);
    const resp = await target.fetch(inner);
    const outcome = cold ? "cold" : "warm";
    lastOutcome.set(key, outcome);
    logEvent({
      requestId,
      deploymentId,
      loaderKeyHash,
      loaderOutcome: outcome,
      dispatchKind: kind,
      entrypoint: entrypoint || "default",
      bindingType,
      resourceIdHash,
      durationMs: Date.now() - started,
      outcome: resp.ok ? "ok" : "error",
      errorCode: resp.ok ? null : `HTTP_${resp.status}`,
    });
    const headers = new Headers(resp.headers);
    headers.set("x-g0-loader-outcome", outcome);
    headers.set("x-g0-request-id", requestId);
    return new Response(resp.body, { status: resp.status, headers });
  } catch (err) {
    const classified = classifyThrown(err);
    logEvent({
      requestId,
      deploymentId,
      loaderKeyHash,
      loaderOutcome: "error",
      dispatchKind: kind,
      entrypoint,
      bindingType,
      resourceIdHash,
      durationMs: Date.now() - started,
      outcome: "error",
      errorCode: classified.errorCode,
    });
    return tenantError(classified.errorCode, requestId, deploymentId, classified.status);
  }
}

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    const requestId = request.headers.get("x-g0-request-id") || crypto.randomUUID();

    if (url.pathname === "/dispatch") {
      const body = await request.json();
      return dispatch(request, env, ctx, { ...body, requestId });
    }

    if (url.pathname === "/route") {
      if (request.method === "GET") {
        return json({ ok: true, routes: Object.fromEntries(routes) });
      }
      const body = await request.json();
      if (!body.accountId || !body.workerId || !body.deploymentId) {
        return tenantError("ROUTE_NOT_SET", requestId, body.deploymentId ?? null, 400);
      }
      const key = `${body.accountId}/${body.workerId}`;
      routes.set(key, body.deploymentId);
      return json({ ok: true, active: body.deploymentId });
    }

    if (url.pathname === "/active") {
      const body = await request.json();
      const routeKey = `${body.accountId}/${body.workerId}`;
      const deploymentId = routes.get(routeKey);
      if (!deploymentId) {
        return tenantError("ROUTE_NOT_SET", requestId, null, 404);
      }
      return dispatch(request, env, ctx, { ...body, requestId, deploymentId });
    }

    if (url.pathname === "/stats") {
      return json({
        ok: true,
        callbacks: Object.fromEntries(callbackCounts),
        lastOutcome: Object.fromEntries(lastOutcome),
        hashes: Object.fromEntries(
          [...callbackCounts.keys()].map((key) => [key, getSeenHash(key)])
        ),
      });
    }

    if (url.pathname === "/invariant") {
      const body = await request.json();
      const key = body.key;
      if (!key || !getDeployment(key)) {
        return tenantError("DEPLOYMENT_NOT_FOUND", requestId, null, 404);
      }
      try {
        const current = getSeenHash(key);
        const attempted = await fingerprintWorkerCode(env, key, {
          alternateRoot: body.alternateRoot,
        });
        if (current && current !== attempted.hash) {
          logEvent({
            requestId,
            loaderKeyHash: await sha256hex(key),
            loaderOutcome: "error",
            outcome: "error",
            errorCode: "PLATFORM_INVARIANT_VIOLATION",
          });
          return json(
            {
              ok: false,
              errorCode: "PLATFORM_INVARIANT_VIOLATION",
              classification: "platform-invariant-violation",
            },
            409
          );
        }
        return json({
          ok: true,
          seen: current != null,
          matched: current != null && current === attempted.hash,
        });
      } catch (err) {
        const classified = classifyThrown(err);
        if (classified.errorCode === "PLATFORM_INVARIANT_VIOLATION") {
          return json(
            {
              ok: false,
              errorCode: "PLATFORM_INVARIANT_VIOLATION",
              classification: "platform-invariant-violation",
            },
            409
          );
        }
        return tenantError(classified.errorCode, requestId, null, classified.status);
      }
    }

    if (url.pathname === "/fault") {
      const body = await request.json();
      faults[body.point] = Boolean(body.enabled);
      return json({ ok: true, faults });
    }

    return tenantError("NOT_PUBLIC", requestId, null, 404);
  },
};
