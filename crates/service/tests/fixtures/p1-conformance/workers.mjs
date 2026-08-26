export function checkWorkersSurface() {
  const request = new Request("https://fixture.invalid/path", {
    method: "POST",
    headers: [["x-p1", "workers"]],
    body: "payload",
  });
  const response = new Response("ok", { status: 201 });
  const clone = structuredClone({ bytes: new Uint8Array([1, 2, 3]), nested: { ok: true } });
  return request.method === "POST"
    && request.headers.get("x-p1") === "workers"
    && response.status === 201
    && clone.bytes instanceof Uint8Array
    && clone.nested.ok === true
    && typeof ReadableStream === "function"
    && typeof AbortController === "function";
}
