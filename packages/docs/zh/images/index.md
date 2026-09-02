# Images

Images 在运行 `ocd` 的主机上对请求体中的图像执行变换，并受尺寸与并发限制。这不是 Cloudflare 托管 Images：不提供图像库、上传签名或 URL 变换。

例如：

- 缩放请求体中的图像
- 叠加图像，输出 jpeg / png / webp / avif
- 使用 `info()` 读取格式信息

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

在 `wrangler.jsonc` 中声明。Images 不是 `bindings` 中的资源 ID，使用顶层字段：

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

语法见 [绑定](/zh/workers/configuration/bindings)。CLI：`oc` / `oc deploy` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images 调用链 | 相同：`input` → `transform` / `draw` → `output` → `response()`，以及 `info()` |
| 产品形态 | 托管 Cloudflare Images | 对本机请求体中的图像做变换 |
| 输入 | 托管图像 ID 或 URL | `ReadableStream` 字节 |
| 上传 / 签名 | 提供 | 不提供 |
| URL 变换 | 提供 | 不提供 |
| 视频 | 提供 | 不提供 |
| AI 放大 | 提供 | 不提供 |
| 配置 | wrangler `images` | `"images": { "binding": "IMAGES" }` |

## 本节

- [上手](/zh/images/get-started/)
- [概念](/zh/images/concepts/)
- [指南](/zh/images/guides/)
- [示例](/zh/images/examples/)
- [限制](/zh/images/platform/limits)
- [行为差异](/zh/images/platform/deviations)
