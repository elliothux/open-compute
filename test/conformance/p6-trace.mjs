import { createHash } from "node:crypto";

const SECRET_HEADERS = new Set(["authorization", "cookie", "set-cookie", "x-api-key"]);
const SECRET_KEYS = /(?:token|secret|password|jwt|authorization|cookie)/i;

function digest(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function sanitizeJson(value) {
  if (Array.isArray(value)) return value.map(sanitizeJson);
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).map(([key, item]) =>
    [key, SECRET_KEYS.test(key) ? "<redacted>" : sanitizeJson(item)]));
}

export function sanitizeTrace(record) {
  const headers = {};
  for (const [rawName, rawValue] of Object.entries(record.headers ?? {})) {
    const name = rawName.toLowerCase();
    let value = String(rawValue);
    if (SECRET_HEADERS.has(name)) value = "<redacted>";
    if (name === "content-type") value = value.replace(/boundary=(?:"[^"]+"|[^;\s]+)/i, "boundary=<boundary>");
    headers[name] = value;
  }
  let body;
  if (record.body !== undefined) {
    const bytes = Buffer.isBuffer(record.body) ? record.body : Buffer.from(String(record.body));
    if ((headers["content-type"] ?? "").includes("json")) {
      try { body = sanitizeJson(JSON.parse(bytes.toString("utf8"))); }
      catch { body = { bytes: bytes.length, sha256: digest(bytes) }; }
    } else body = { bytes: bytes.length, sha256: digest(bytes) };
  }
  return {
    method: String(record.method).toUpperCase(),
    path: String(record.path)
      .replaceAll(/[0-9a-f]{32}/g, "{account_id}")
      .replaceAll(/[0-9a-f]{8}-[0-9a-f-]{27,}/gi, "{resource_id}"),
    headers,
    ...(body === undefined ? {} : { body }),
  };
}

export function assertSanitizedTrace(value) {
  const encoded = JSON.stringify(value);
  for (const forbidden of ["Bearer ", "api-token", "signed-upload-token", "super-secret"]) {
    if (encoded.includes(forbidden)) throw new Error(`trace contains unsanitized secret marker: ${forbidden}`);
  }
}
