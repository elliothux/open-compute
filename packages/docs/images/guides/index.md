# 指南

## 声明

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

`fit`：`scale-down` | `contain` | `cover` | `crop` | `pad`。未知选项抛 `IMAGE_OPTION_UNSUPPORTED`。`anim` 只能是省略或 `false`。

不要使用 Cloudflare Images 的 upload API 或 signing。
