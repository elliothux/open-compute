# 概念

引擎在 `ocd` 进程里做有界光栅变换。session 有 TTL 和并发上限。输出不会自动进 Workers Cache；要缓存就显式用 Cache API。

支持输入 jpeg / png / webp（`info`）。输出 jpeg / png / webp / avif。`max_frames = 1`：没有动画。

不提供：

- 上传、签名 URL、Image Delivery
- `https://.../cdn-cgi/image/` URL transform
- 视频
- AI upscale
- Cloudflare 产品配额 / 计费

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Binding API | Images binding 链 | 相同链：`input` / `transform` / `draw` / `output` / `response()` / `info()` |
| 产品 | 托管 Cloudflare Images | 有界本机光栅 binding |
| 上传 / 签名 / URL transform / 视频 / AI upscale | 提供 | 不提供 |
