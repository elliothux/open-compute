# Images

Images is a bounded local raster-transform binding, **not** hosted Cloudflare Images. No URL transforms, no video, no AI upscale, no upload/signing product, no Cloudflare quota page. Do not send people to Cloudflare Images upload / signing docs as if those worked here.

This is a platform product (capability `kind: platform`) with 0 target AST members. Status is `supported_with_deviation`, deviation **`OC-IMAGES-001`**.

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!request.body) return new Response("empty", { status: 400 });
    const out = await env.IMAGES
      .input(request.body)
      .transform({ width: 200, fit: "scale-down" })
      .output({ format: "image/webp" });
    return out.response();
  },
} satisfies ExportedHandler<{ IMAGES: ImagesBinding }>;
```

## What matches Cloudflare

The chain is `input` → `transform` / `draw` → `output` → `response()`, plus `info()`. Input is `ReadableStream` bytes, not a hosted image id. Do not document this as the full Cloudflare Images product.

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

Images is not a resource id in `bindings`. Top-level `"images": { "binding": "IMAGES" }` is enough. See [bindings](/en/workers/configuration/bindings).

## Intentional differences

**`OC-IMAGES-001`**: Images is a bounded local raster transform binding, not hosted Cloudflare Images. Hosted delivery/upload/signing, URL transforms, video, AI upscale, and Cloudflare product quotas are out of scope.

Full text: [Deviations](/en/images/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/images/get-started/)
- [Concepts](/en/images/concepts/)
- [Guides](/en/images/guides/)
- [Examples](/en/images/examples/)
- [Limits](/en/images/platform/limits)
- [Deviations](/en/images/platform/deviations)
