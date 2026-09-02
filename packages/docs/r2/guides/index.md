# Guides

## Create a bucket

See [Get started](/r2/get-started/). `POST /v1/accounts/{accountId}/r2/buckets`. Cloudflare REST is not provided.

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

Object bytes live on the configured provider. You can use that provider's S3 SDK against the same prefix. That is not Cloudflare R2 REST and it does not go through the Worker binding. The Worker path is the API that matches Cloudflare.
