//! Disposable response cache; input credentials appear only in the SHA-256 key.

interface Entry {
  expires: number;
  bytes: ArrayBuffer;
  headers: [string, string][];
}
const entries = new Map<string, Entry>();
let used = 0;

export async function cacheKey(
  action: string,
  options: unknown,
): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(JSON.stringify([action, options])),
  );
  return Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}
function remove(key: string, entry: Entry) {
  used -= entry.bytes.byteLength;
  entries.delete(key);
}
export function cached(key: string, maximum: number): Response | undefined {
  const entry = entries.get(key);
  if (!entry || entry.bytes.byteLength > maximum) return;
  if (performance.now() >= entry.expires) {
    remove(key, entry);
    return;
  }
  const headers = new Headers(entry.headers);
  headers.set("x-browser-ms-used", "0");
  return new Response(entry.bytes.slice(0), { headers });
}
export function remember(
  key: string,
  bytes: ArrayBuffer,
  headers: Headers,
  ttl: number,
  maximum: number,
): void {
  for (const [key, entry] of entries)
    if (performance.now() >= entry.expires) remove(key, entry);
  if (!ttl || bytes.byteLength > maximum) return;
  const existing = entries.get(key);
  if (existing) remove(key, existing);
  // ponytail: bounded insertion-order eviction, replace with LRU only if measurements justify it.
  while (used + bytes.byteLength > maximum || entries.size >= 128) {
    const oldest = entries.entries().next().value;
    if (!oldest) break;
    remove(oldest[0], oldest[1]);
  }
  entries.set(key, {
    expires: performance.now() + ttl * 1000,
    bytes: bytes.slice(0),
    headers: [...headers],
  });
  used += bytes.byteLength;
}
