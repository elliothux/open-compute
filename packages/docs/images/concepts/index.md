# Concepts

The engine runs bounded raster transforms inside the `ocd` process. Sessions have a TTL and a concurrency cap. Output is not automatically stored in Workers Cache; cache it explicitly with the Cache API if you need that.

Input formats for `info` are jpeg / png / webp. Output is jpeg / png / webp / avif. `max_frames = 1`: no animation.

Not provided:

- upload, signed URLs, Image Delivery
- `https://.../cdn-cgi/image/` URL transforms
- video
- AI upscale
- Cloudflare product quotas / billing

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images binding chain | Same chain: `input` / `transform` / `draw` / `output` / `response()` / `info()` |
| Product | Hosted Cloudflare Images | Bounded local raster binding |
| Upload / signing / URL transform / video / AI upscale | Available | Not provided |
