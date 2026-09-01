# Guides

## Declare

```json
"images": { "binding": "IMAGES" }
```

## transform / draw / output

```ts
const result = await env.IMAGES
  .input(body)
  .transform({ width: 200, height: 200, fit: "cover", gravity: "center", rotate: 90, flip: "h", blur: 2 })
  .draw(overlay, { left: 8, top: 8, opacity: 0.8, composite: "over" })
  .output({ format: "image/jpeg", quality: 70 });
return result.response({ headers: { "cache-control": "public, max-age=3600" } });
```

`fit`: `scale-down` | `contain` | `cover` | `crop` | `pad`. Unknown options throw `IMAGE_OPTION_UNSUPPORTED`. `anim` is omit or `false`.

Do not use the Cloudflare Images upload API or signing.
