# Examples

POST raw bytes to the Worker and run `input` → `transform` → `output`. Do not construct an `imagedelivery.net` or `/cdn-cgi/image/` URL.

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

The chain is `input` / `transform` / `draw` / `output` / `response()` / `info()`. Options: [Guides](/en/images/guides/). This is a bounded local raster transform binding, not hosted Cloudflare Images. Setup: [Get started](/en/images/get-started/).
