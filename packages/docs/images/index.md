# Images

Images 是有界的本机光栅变换 binding。输入是请求体字节；变换在运行 ocd 的该节点上完成。不是托管的 Cloudflare Images。

例如：

- 缩放与转换请求体中的图像
- overlay 并输出 jpeg / png / webp / avif
- 用 `info()` 读取格式

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

在 `open-compute.json` 中声明。Images 不是 `bindings` 里的资源 ID，只用顶层 `"images": { "binding": "IMAGES" }`：

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images binding 链 | 相同链：`input` → `transform` / `draw` → `output` → `response()`，以及 `info()` |
| 产品 | 托管 Cloudflare Images | 有界本机光栅 binding |
| 输入 | 托管图像 ID 或 URL | `ReadableStream` 字节 |
| 上传 / 签名 | 提供 | 不提供 |
| URL transform | 提供 | 不提供 |
| 视频 | 提供 | 不提供 |
| AI upscale | 提供 | 不提供 |
| Binding | wrangler `images` | `"images": { "binding": "IMAGES" }` |

## 本节

- [上手](/images/get-started/)
- [概念](/images/concepts/)
- [指南](/images/guides/)
- [示例](/images/examples/)
- [限制](/images/platform/limits)
- [行为差异](/images/platform/deviations)
