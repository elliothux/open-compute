# R2

R2 是绑到 Worker `env` 的对象存储。Worker binding API 与 Cloudflare 相同；对象字节由你配置的 S3-compatible provider 持有。没有 Cloudflare 全球 placement。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const key = url.pathname.slice(1);
    if (request.method === "PUT") {
      await env.BUCKET.put(key, request.body);
      return new Response("ok");
    }
    const object = await env.BUCKET.get(key);
    if (object === null) return new Response("missing", { status: 404 });
    return new Response(object.body, { headers: { "etag": object.httpEtag } });
  },
} satisfies ExportedHandler<{ BUCKET: R2Bucket }>;
```

## 与 Cloudflare 相同

Worker binding 与 [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、multipart、HTTP metadata。110 个目标成员为 `supported_with_deviation`。对象字节也可以走你配置的 S3-compatible API（provider 自己的 SDK），那是存储协议，不是另一套 Cloudflare REST。

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<r2-bucket-id>" }
  }
}
```

`id` 是平台上已存在的逻辑 bucket。绑定语法见 [bindings](/workers/configuration/bindings)。不要从本页抄 `client.v4`。

## 故意不同

**`OC-R2-001`**：R2 object bytes 由配置的 S3-compatible provider 持有。不声称 Cloudflare 全球 placement 或 replication。没有智能就近、没有 Cloudflare 托管的公开 r2.dev 产品。

全文见 [偏差](/r2/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/r2/get-started/)
- [概念](/r2/concepts/)
- [指南](/r2/guides/)
- [示例](/r2/examples/)
- [限制](/r2/platform/limits)
- [偏差](/r2/platform/deviations)
