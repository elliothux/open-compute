const READY_PATH = "/internal/ready";
const TOKEN_HEADER = "x-open-compute-internal-token";
const INTERNAL_PATHS = new Set([
  "/internal/dispatch",
  "/internal/validate",
  "/internal/validate-do",
]);
const DO_ADMIN_PATH = "/internal/do-delete";
const DO_ALARM_PATHS = new Set(["/internal/do-alarm", "/internal/do-alarm-repair"]);

function tokenEquals(left, right) {
  const encoder = new TextEncoder();
  const a = encoder.encode(String(left || ""));
  const b = encoder.encode(String(right || ""));
  const n = a.length > b.length ? a.length : b.length;
  let diff = a.length ^ b.length;
  for (let i = 0; i < n; i++) {
    const av = i < a.length ? a[i] : 0;
    const bv = i < b.length ? b[i] : 0;
    diff |= av ^ bv;
  }
  return diff === 0;
}

function deny() {
  return new Response(null, { status: 404 });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    const presented = request.headers.get(TOKEN_HEADER);
    if (!tokenEquals(presented, env.INTERNAL_TOKEN)) {
      return deny();
    }
    if (request.method === "GET" && url.pathname === READY_PATH && url.search === "") {
      if (request.headers.has("content-type")) return deny();
      const length = request.headers.get("content-length");
      if (length !== null && length !== "0") return deny();
      const body = await request.arrayBuffer();
      return body.byteLength === 0 ? new Response(null, { status: 204 }) : deny();
    }
    if (request.method !== "POST" || !INTERNAL_PATHS.has(url.pathname) || url.search !== "") {
      if (request.method === "POST" && DO_ALARM_PATHS.has(url.pathname) && url.search === "") {
        return env.DO_ROUTER.fetch(new Request(`http://do-router${url.pathname}`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: request.body,
        }));
      }
      if (request.method === "POST" && url.pathname === DO_ADMIN_PATH && url.search === "") {
        return env.DO_ROUTER.fetch(new Request("http://do-router/internal/do-delete", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: request.body,
        }));
      }
      return deny();
    }
    const headers = new Headers(request.headers);
    // Forward only the authenticated generation token to the platform-owned
    // loader host. The host removes it before constructing the tenant Request.
    headers.set(TOKEN_HEADER, env.INTERNAL_TOKEN);
    return env.LOADER_HOST.fetch(new Request(`http://loader-host${url.pathname}`, {
      method: "POST",
      headers,
      body: request.body,
      redirect: "manual",
    }));
  },
};
