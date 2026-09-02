# 上手

没有资源可创建：Images 是部署级 binding，不引用 namespace id。`ocd` 必须就绪。

## 1. 声明 binding

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

不要写进 `bindings`，也不要提供 Cloudflare Images 账户 ID。

```sh
bun run oc types --config open-compute.json
```

## 2. Worker

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!request.body) return new Response("empty", { status: 400 });
    const info = await env.IMAGES.info(request.body);
    // info.format 是 jpeg | png | webp
    const out = await env.IMAGES
      .input(request.body)
      .transform({ width: 320, fit: "contain" })
      .output({ format: "image/webp", quality: 80 });
    return out.response();
  },
} satisfies ExportedHandler<Env>;
```

输入必须是请求体字节。不提供 `https://imagedelivery.net/...` URL transform，也不提供上传 / 签名。

## 3. 运行

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

CLI 为 `oc`，不是 Wrangler。下一步：[概念](/zh/images/concepts/)。
