# 示例

把原始字节 POST 到 Worker，走 `input` → `transform` → `output`。不构造 `imagedelivery.net` 或 `/cdn-cgi/image/` URL。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!request.body) return new Response("empty", { status: 400 });
    const out = await env.IMAGES
      .input(request.body)
      .transform({ width: 320, fit: "scale-down" })
      .output({ format: "image/webp", quality: 80 });
    return out.response();
  },
} satisfies ExportedHandler<{ IMAGES: ImagesBinding }>;
```

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

链是 `input` / `transform` / `draw` / `output` / `response()` / `info()`。完整选项见[指南](/images/guides/)。这是有界本机光栅变换 binding，不是托管 Cloudflare Images。配置见[上手](/images/get-started/)。
