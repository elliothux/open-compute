# 示例

用 Worker binding 读写对象。对象字节落在配置的 Local 或 S3 authority，Worker 代码不随 backend 改变。

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

配置见[上手](/zh/r2/get-started/)。
