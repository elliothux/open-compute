# Images

Images is a bounded local raster-transform binding. Input is request-body bytes. Transforms run on the node running ocd. This is not hosted Cloudflare Images.

For example, you can use Images for:

- Resizing and converting request-body images
- Overlay and output jpeg / png / webp / avif
- Inspecting format with `info()`

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

Declare it in `open-compute.json`. Images is not a resource id in `bindings`. Use top-level `"images": { "binding": "IMAGES" }` only:

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

See [bindings](/workers/configuration/bindings). The CLI is `oc` / `oc run` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images binding chain | Same chain: `input` → `transform` / `draw` → `output` → `response()`, plus `info()` |
| Product | Hosted Cloudflare Images | Bounded local raster binding |
| Input | Hosted image id or URL | `ReadableStream` bytes |
| Upload / signing | Available | Not provided |
| URL transform | Available | Not provided |
| Video | Available | Not provided |
| AI upscale | Available | Not provided |
| Binding | wrangler `images` | `"images": { "binding": "IMAGES" }` |

## Next

- [Get started](/images/get-started/)
- [Concepts](/images/concepts/)
- [Guides](/images/guides/)
- [Examples](/images/examples/)
- [Limits](/images/platform/limits)
- [Behavior differences](/images/platform/deviations)
