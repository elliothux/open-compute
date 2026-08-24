const TENANT_SAFE = new Set([
  "DISPATCH_KIND_UNSUPPORTED",
  "DISPATCH_KIND_UNKNOWN",
  "ENTRYPOINT_NOT_FOUND",
  "DEPLOYMENT_NOT_FOUND",
  "LOADER_ERROR",
  "BUNDLE_INVALID",
  "OUTBOUND_DENIED",
  "BINDING_DENIED",
  "BINDING_INTERNAL",
  "BINDING_TYPE",
  "IDENTITY_IMMUTABLE",
  "FACET_NOT_FOUND",
  "IDENTIFIER_INVALID",
  "CLASS_NOT_FOUND",
  "DO_ERROR",
  "PLATFORM_INVARIANT_VIOLATION",
  "FAULT_INJECTED",
  "ROUTE_NOT_SET",
  "NOT_PUBLIC",
  "NOT_FOUND",
  "INTERNAL",
]);

export function tenantError(errorCode, requestId, deploymentId, status = 400) {
  const code = TENANT_SAFE.has(errorCode) ? errorCode : "INTERNAL";
  return Response.json(
    {
      ok: false,
      errorCode: code,
      requestId: requestId ?? null,
      deploymentId: deploymentId ?? null,
    },
    { status }
  );
}

export function classifyThrown(err) {
  const message = String(err && err.message ? err.message : err);
  const name = String(err && err.name ? err.name : "");
  if (err && err.errorCode && TENANT_SAFE.has(err.errorCode)) {
    if (err.errorCode === "PLATFORM_INVARIANT_VIOLATION") {
      return { errorCode: err.errorCode, status: 500 };
    }
    if (err.errorCode === "DEPLOYMENT_NOT_FOUND" || err.errorCode === "ENTRYPOINT_NOT_FOUND") {
      return { errorCode: err.errorCode, status: 404 };
    }
    if (err.errorCode === "CLASS_NOT_FOUND") {
      return { errorCode: err.errorCode, status: 404 };
    }
    if (err.errorCode === "IDENTIFIER_INVALID" || err.errorCode === "BUNDLE_INVALID") {
      return { errorCode: err.errorCode, status: 400 };
    }
    return { errorCode: err.errorCode, status: 500 };
  }
  if (message.includes("PLATFORM_INVARIANT_VIOLATION")) {
    return { errorCode: "PLATFORM_INVARIANT_VIOLATION", status: 500 };
  }
  if (
    /does not export a Durable Object class named/i.test(message) ||
    /ActorClassChannel is not ready yet/i.test(message)
  ) {
    return { errorCode: "CLASS_NOT_FOUND", status: 404 };
  }
  if (
    /no such entrypoint/i.test(message) ||
    /entrypoint name .+ was not found in this worker/i.test(message) ||
    /refers to a Durable Object class/i.test(message)
  ) {
    return { errorCode: "ENTRYPOINT_NOT_FOUND", status: 404 };
  }
  if (message.includes("not permitted to access the internet")) {
    return { errorCode: "OUTBOUND_DENIED", status: 403 };
  }
  if (message.includes("FAULT_INJECTED")) {
    return { errorCode: "FAULT_INJECTED", status: 500 };
  }
  if (
    /FIXTURE_NOT_FOUND|syntax|parse|unexpected|module not found|no such module|could not resolve|does not exist/i.test(
      message
    )
  ) {
    return { errorCode: "BUNDLE_INVALID", status: 400 };
  }
  if (name === "AbortError" || /aborted|canceled this request/i.test(message)) {
    return { errorCode: "LOADER_ERROR", status: 500 };
  }
  return { errorCode: "LOADER_ERROR", status: 500 };
}

export function sanitizeThrown(err) {
  const message = String(err && err.message ? err.message : "error");
  return message
    .replace(/\/[^\s:]+/g, "[path]")
    .replace(/secret[^\s]*/gi, "[redacted]")
    .slice(0, 200);
}
