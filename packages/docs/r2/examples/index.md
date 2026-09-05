# Examples

Read and write objects through the Worker binding. Object bytes live on the configured Local or S3 authority without changing Worker code.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const key = new URL(request.url).pathname.slice(1) || "index.txt";
    if (request.method === "PUT") {
      const uploaded = await env.BUCKET.put(key, request.body, {
        httpMetadata: { contentType: request.headers.get("content-type") ?? "application/octet-stream" },
      });
      return Response.json({ key, etag: uploaded.httpEtag });
    }
    const object = await env.BUCKET.get(key);
    if (!object) return new Response("missing", { status: 404 });
    const headers = new Headers();
    headers.set("etag", object.httpEtag);
    if (object.httpMetadata?.contentType) {
      headers.set("content-type", object.httpMetadata.contentType);
    }
    return new Response(object.body, { headers });
  },
} satisfies ExportedHandler<Env>;
```

Config: [Get started](/r2/get-started/).
