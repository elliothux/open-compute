# 指南

## 创建 bucket

见[上手](/zh/r2/get-started/)。bucket CRUD 通过官方 SDK 或固定 Wrangler 使用 `/client/v4/accounts/{account_id}/r2/buckets`。

## Worker 读写

```ts
await env.BUCKET.put("notes/1.txt", "hello", {
  httpMetadata: { contentType: "text/plain" },
});
const hit = await env.BUCKET.get("notes/1.txt");
const listed = await env.BUCKET.list({ prefix: "notes/" });
await env.BUCKET.delete("notes/1.txt");
```

完整选项见 [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/)。

## S3-compatible

本节只适用于整个平台选择 S3 backend 的情况。provider API 不是 Cloudflare R2 REST，也不走 Worker binding。Local 没有公开 S3 endpoint，其文件是平台内部认证格式，不能直接编辑。上面的 Worker 路径在两种 backend 上都是与 Cloudflare 对齐的 API。
