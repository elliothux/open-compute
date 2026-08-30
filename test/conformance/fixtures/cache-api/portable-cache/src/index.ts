export default {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const key = new Request(new URL("/portable-cache/object", url), { method: "GET" });
    const cache = caches.default;
    if (url.pathname === "/reset") {
      await cache.delete(key);
      return Response.json({ reset: true });
    }
    if (url.pathname !== "/probe") return new Response("not found", { status: 404 });
    const cached = await cache.match(key);
    if (cached !== undefined) {
      return Response.json({ cache: "HIT", body: await cached.text() });
    }
    const body = "portable-cache-v1";
    await cache.put(key, new Response(body, {
      headers: { "cache-control": "public, max-age=300", etag: '"portable-cache-v1"' },
    }));
    return Response.json({ cache: "MISS", body });
  },
} satisfies ExportedHandler;
