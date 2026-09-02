# Behavior differences

Images is a bounded local raster transform binding, not hosted Cloudflare Images.

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

See [Compatibility](/platform/compatibility).
