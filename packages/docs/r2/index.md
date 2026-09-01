# R2

R2 是对象存储，用于从 Worker 读写非结构化对象。Worker binding API 与 Cloudflare 对齐；对象字节由配置的 S3-compatible provider 持有。

例如：

- 存储非结构化对象
- 从 Worker 读写文件
- 对配置的 S3 provider 做 multipart 上传

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

在 `open-compute.json` 中绑定已有的逻辑 bucket。普通产品 binding 为 `{ type, id, permissions? }`：

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<r2-bucket-id>" }
  }
}
```

`id` 是本平台上已存在的逻辑 bucket。绑定语法见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、multipart、HTTP metadata |
| 对象字节 | Cloudflare R2 存储 | 配置的 S3-compatible provider |
| 全球 placement | 提供 | 不提供 |
| r2.dev 公开产品 | 提供 | 不提供 |
| Jurisdictional restrictions | 提供 | 不提供 |
| REST / `client.v4` | 提供 | 不提供；使用 Worker binding 或 provider 的 S3 API |

## 本节

- [上手](/r2/get-started/)
- [概念](/r2/concepts/)
- [指南](/r2/guides/)
- [示例](/r2/examples/)
- [限制](/r2/platform/limits)
- [行为差异](/r2/platform/deviations)
