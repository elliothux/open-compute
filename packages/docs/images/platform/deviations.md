# 行为差异

Images 对本机请求体中的图像做变换，并受尺寸与并发限制。这不是 Cloudflare 托管 Images。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images binding 链 | 相同链：`input` → `transform` / `draw` → `output` → `response()`，以及 `info()` |
| 产品 | 托管 Cloudflare Images | 对本机请求体中的图像做变换 |
| 输入 | 托管图像 ID 或 URL | `ReadableStream` 字节 |
| 上传 / 签名 | 提供 | 不提供 |
| URL transform | 提供 | 不提供 |
| 视频 | 提供 | 不提供 |
| AI upscale | 提供 | 不提供 |
| Binding | wrangler `images` | `"images": { "binding": "IMAGES" }` |

见[兼容性](/platform/compatibility)。
