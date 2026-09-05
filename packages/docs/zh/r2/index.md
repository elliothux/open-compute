# R2

R2 是 Worker 可访问的对象存储。Worker 绑定 API 与 Cloudflare 一致；对象数据由 operator 为整个平台选定的 Local 或 S3 backend 持有。

例如：

- 存储文件与二进制对象
- 从 Worker 读写对象
- 在任一受支持 backend 上执行分片上传

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

在 `wrangler.jsonc` 中绑定已存在的 bucket：

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "r2_buckets": [{ "binding": "BUCKET", "bucket_name": "files" }]
}
```

`bucket_name` 必须指向 account 中已有的逻辑 bucket。语法见[绑定](/zh/workers/configuration/bindings)。bucket 与 object 操作使用固定 Wrangler 或官方 SDK。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、分片上传、HTTP metadata |
| 对象存储位置 | Cloudflare R2 | 单节点上配置的 Local 或 S3 authority |
| 全球就近存放 | 提供 | 不提供 |
| r2.dev 公开访问 | 提供 | 不提供 |
| 数据驻留限制 | 提供 | 不提供 |
| REST / `client/v4` | 提供 | 兼容 account-scoped bucket 与 object 操作 |

## 本节

- [上手](/zh/r2/get-started/)
- [概念](/zh/r2/concepts/)
- [指南](/zh/r2/guides/)
- [示例](/zh/r2/examples/)
- [限制](/zh/r2/platform/limits)
- [行为差异](/zh/r2/platform/deviations)
