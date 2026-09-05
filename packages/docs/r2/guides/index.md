# Guides

## Create a bucket

See [Get started](/r2/get-started/). Bucket CRUD uses `/client/v4/accounts/{account_id}/r2/buckets` through the official SDK or pinned Wrangler.

## Worker reads and writes

```ts
await env.BUCKET.put("notes/1.txt", "hello", {
  httpMetadata: { contentType: "text/plain" },
});
const hit = await env.BUCKET.get("notes/1.txt");
const listed = await env.BUCKET.list({ prefix: "notes/" });
await env.BUCKET.delete("notes/1.txt");
```

Full options: [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/).

## S3-compatible

This section applies only when the platform-wide object backend is S3. Its provider API is not Cloudflare R2 REST and does not go through the Worker binding. Local has no public S3 endpoint and its files are an internal authenticated format; do not edit them directly. The Worker path above is the API that matches Cloudflare on both backends.
