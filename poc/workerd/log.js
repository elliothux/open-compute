const FORBIDDEN = [
  "secret",
  "password",
  "credential",
  "/users/",
  "/var/",
  "/tmp/",
  "stack",
];

function sanitizeMeta(meta) {
  if (!meta || typeof meta !== "object") return {};
  const out = {};
  for (const [key, value] of Object.entries(meta)) {
    if (value == null) {
      out[key] = value;
      continue;
    }
    if (typeof value === "string") {
      let next = value;
      for (const token of FORBIDDEN) {
        if (next.toLowerCase().includes(token)) next = "[redacted]";
      }
      out[key] = next;
    } else {
      out[key] = value;
    }
  }
  return out;
}

export function logEvent(fields) {
  const entry = {
    timestamp: new Date().toISOString(),
    requestId: fields.requestId ?? null,
    workerdPid: fields.workerdPid ?? null,
    deploymentId: fields.deploymentId ?? null,
    loaderKeyHash: fields.loaderKeyHash ?? null,
    loaderOutcome: fields.loaderOutcome ?? null,
    dispatchKind: fields.dispatchKind ?? null,
    entrypoint: fields.entrypoint ?? null,
    bindingType: fields.bindingType ?? null,
    resourceIdHash: fields.resourceIdHash ?? null,
    doStorageIdHash: fields.doStorageIdHash ?? null,
    className: fields.className ?? null,
    objectIdHash: fields.objectIdHash ?? null,
    durationMs: fields.durationMs ?? null,
    outcome: fields.outcome ?? null,
    errorCode: fields.errorCode ?? null,
    ...sanitizeMeta(fields.extra ?? {}),
  };
  console.log(JSON.stringify(entry));
}

export async function sha256hex(value) {
  const bytes = new TextEncoder().encode(String(value));
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export function requestIdFrom(request, fallback) {
  return request.headers.get("x-g0-request-id") || fallback || crypto.randomUUID();
}
