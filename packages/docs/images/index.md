# Images

Images 是有界的本机光栅变换 binding，**不是**托管的 Cloudflare Images。没有 URL transform、没有视频、没有 AI upscale、没有上传/签名产品、没有 Cloudflare 配额页。不要把人送到 Cloudflare Images 的 upload / signing 文档当本平台能力。

这是平台产品（capability `kind: platform`），目标 AST 成员数为 0。状态为 `supported_with_deviation`，偏差 **`OC-IMAGES-001`**。

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

## 与 Cloudflare 相同的部分

链是 `input` → `transform` / `draw` → `output` → `response()`，以及 `info()`。输入是 `ReadableStream` 字节，不是托管图像 ID。不要把它写成完整 Cloudflare Images 产品。

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

Images 不是 `bindings` 里的资源 ID。顶层 `"images": { "binding": "IMAGES" }` 即可。见 [bindings](/workers/configuration/bindings)。

## 故意不同

**`OC-IMAGES-001`**：Images 是有界的本机光栅变换 binding，不是托管的 Cloudflare Images。托管投递/上传/签名、URL transform、视频、AI upscale 和 Cloudflare 产品配额不在范围内。

全文见 [偏差](/images/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/images/get-started/)
- [概念](/images/concepts/)
- [指南](/images/guides/)
- [示例](/images/examples/)
- [限制](/images/platform/limits)
- [偏差](/images/platform/deviations)
