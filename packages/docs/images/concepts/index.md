# 概念

引擎在 `ocd` 进程里做有界光栅变换。session 有 TTL 和并发上限。输出不会自动进 Workers Cache；要缓存就显式用 Cache API。

支持输入 jpeg / png / webp（`info`）。输出 jpeg / png / webp / avif。`max_frames = 1`：没有动画。

## 不是托管 Cloudflare Images

没有：

- 上传、签名 URL、Image Delivery
- `https://.../cdn-cgi/image/` URL transform
- 视频
- AI upscale
- Cloudflare 产品配额 / 计费

不要把读者送到那些 Cloudflare 文档当本平台能力。

## 故意不同

**`OC-IMAGES-001`**：有界本机光栅变换 binding，不是托管 Cloudflare Images。
