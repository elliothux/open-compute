import { tenantError } from "./errors.js";
import { logEvent, requestIdFrom } from "./log.js";

const INTERNAL_PATHS = new Set([
  "/loader-host",
  "/binding-host",
  "/do-supervisor",
  "/g0-do-disk",
  "/g0-fixtures",
  "/internal",
  "/admin",
  "/debug",
]);

function isInternalPath(pathname) {
  if (INTERNAL_PATHS.has(pathname)) return true;
  return (
    pathname.startsWith("/internal/") ||
    pathname.startsWith("/loader-host") ||
    pathname.startsWith("/binding-host") ||
    pathname.startsWith("/do-supervisor")
  );
}

function hostRequestId() {
  return crypto.randomUUID();
}

function stripIdentityHeaders(headers) {
  const out = { ...(headers || {}) };
  delete out["x-account-id"];
  delete out["x-deployment-id"];
  delete out["X-Account-Id"];
  delete out["X-Deployment-Id"];
  delete out["x-worker-id"];
  return out;
}

function fetchInit(request, init) {
  const next = { ...init };
  if (request.signal) {
    next.signal = request.signal;
  }
  return next;
}

async function proxy(service, request, path, init = {}) {
  const url = new URL(path, "http://g0-internal");
  return service.fetch(
    url.toString(),
    fetchInit(request, {
      method: init.method || request.method,
      headers: init.headers || request.headers,
      body:
        init.body === undefined
          ? request.method === "GET" || request.method === "HEAD"
            ? undefined
            : request.body
          : init.body,
    })
  );
}

export default {
  async fetch(request, env) {
    const started = Date.now();
    const url = new URL(request.url);
    const requestId = requestIdFrom(request);

    try {
      if (url.pathname === "/health") {
        logEvent({
          requestId,
          dispatchKind: "health",
          durationMs: Date.now() - started,
          outcome: "ok",
        });
        return Response.json({ ok: true, service: "ingress" });
      }

      if (isInternalPath(url.pathname)) {
        logEvent({
          requestId,
          dispatchKind: "blocked-internal",
          durationMs: Date.now() - started,
          outcome: "error",
          errorCode: "NOT_PUBLIC",
        });
        return tenantError("NOT_PUBLIC", requestId, null, 404);
      }

      if (url.pathname === "/echo") {
        return proxy(env.ECHO, request, "/");
      }
      if (url.pathname === "/echo/named") {
        return proxy(env.ECHO_NAMED, request, "/");
      }
      if (url.pathname === "/echo/throw") {
        return proxy(env.ECHO, request, "/throw");
      }

      if (url.pathname === "/g0/dispatch") {
        const body = await request.json();
        const dispatchId = hostRequestId();
        return env.LOADER_HOST.fetch(
          "http://loader-host/dispatch",
          fetchInit(request, {
            method: "POST",
            headers: { "content-type": "application/json", "x-g0-request-id": dispatchId },
            body: JSON.stringify({
              kind: body.kind ?? "fetch",
              accountId: body.accountId,
              workerId: body.workerId,
              deploymentId: body.deploymentId,
              entrypoint: body.entrypoint ?? null,
              requestId: dispatchId,
              method: body.method,
              url: body.url,
              headers: stripIdentityHeaders(body.headers),
              body: body.body ?? null,
            }),
          })
        );
      }

      if (url.pathname === "/g0/route") {
        return proxy(env.LOADER_HOST, request, "/route", {
          headers: { "content-type": "application/json", "x-g0-request-id": requestId },
        });
      }

      if (url.pathname === "/g0/active") {
        const body = await request.json();
        const dispatchId = hostRequestId();
        return env.LOADER_HOST.fetch(
          "http://loader-host/active",
          fetchInit(request, {
            method: "POST",
            headers: { "content-type": "application/json", "x-g0-request-id": dispatchId },
            body: JSON.stringify({
              kind: body.kind ?? "fetch",
              accountId: body.accountId,
              workerId: body.workerId,
              entrypoint: body.entrypoint ?? null,
              requestId: dispatchId,
              method: body.method,
              url: body.url,
              headers: stripIdentityHeaders(body.headers),
              body: body.body ?? null,
            }),
          })
        );
      }

      if (url.pathname === "/g0/loader/stats") {
        return env.LOADER_HOST.fetch("http://loader-host/stats", {
          headers: { "x-g0-request-id": requestId },
        });
      }

      if (url.pathname === "/g0/loader/invariant") {
        return proxy(env.LOADER_HOST, request, "/invariant", {
          headers: { "content-type": "application/json", "x-g0-request-id": requestId },
        });
      }

      if (url.pathname === "/g0/do") {
        return proxy(env.DO_SUPERVISOR, request, "/", {
          headers: { "content-type": "application/json", "x-g0-request-id": requestId },
        });
      }

      if (url.pathname === "/g0/fault") {
        const body = await request.json();
        const target = body.target || "loader";
        const payload = JSON.stringify({
          point: body.point,
          enabled: body.enabled !== false,
          resourceId: body.resourceId ?? null,
        });
        if (target === "binding") {
          return env.BINDING_HOST.fetch("http://binding-host/fault", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: payload,
          });
        }
        if (target === "do") {
          return env.DO_SUPERVISOR.fetch("http://do-supervisor/", {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ op: "fault", point: body.point, enabled: body.enabled !== false }),
          });
        }
        return env.LOADER_HOST.fetch("http://loader-host/fault", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: payload,
        });
      }

      if (url.pathname === "/g0/throw") {
        throw new Error("g0-ingress-throw");
      }

      logEvent({
        requestId,
        dispatchKind: "unknown",
        durationMs: Date.now() - started,
        outcome: "error",
        errorCode: "NOT_FOUND",
      });
      return tenantError("NOT_FOUND", requestId, null, 404);
    } catch (err) {
      if (request.signal?.aborted || (err && err.name === "AbortError")) {
        throw err;
      }
      logEvent({
        requestId,
        dispatchKind: "ingress",
        durationMs: Date.now() - started,
        outcome: "error",
        errorCode: "INTERNAL",
      });
      return tenantError("INTERNAL", requestId, null, 500);
    }
  },
};
