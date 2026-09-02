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

对象字节在配置的 provider 上。可以用该 provider 的 S3 SDK 操作同一 prefix。那不是 Cloudflare R2 REST，也不走 Worker binding。Worker 路径才是与 Cloudflare 对齐的 API。
