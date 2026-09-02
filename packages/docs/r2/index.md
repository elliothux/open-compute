# R2

R2 是 Worker 可访问的对象存储。Worker 绑定 API 与 Cloudflare 一致；对象数据存放在配置的 S3 兼容存储中。

例如：

- 存储文件与二进制对象
- 从 Worker 读写对象
- 通过配置的 S3 执行分片上传

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

在 `open-compute.json` 中绑定已存在的 bucket：

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<r2-bucket-id>" }
  }
}
```

`id` 必须指向平台上已有的逻辑 bucket。语法见 [绑定](/workers/configuration/bindings)。CLI：`oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | 相同：`head` / `get` / `put` / `delete` / `list`、条件写、checksum、分片上传、HTTP metadata |
| 对象存储位置 | Cloudflare R2 | 配置的 S3 兼容存储 |
| 全球就近存放 | 提供 | 不提供 |
| r2.dev 公开访问 | 提供 | 不提供 |
| 数据驻留限制 | 提供 | 不提供 |
| REST / `client.v4` | 提供 | 不提供；使用 Worker 绑定或存储商的 S3 API |

## 本节

- [上手](/r2/get-started/)
- [概念](/r2/concepts/)
- [指南](/r2/guides/)
- [示例](/r2/examples/)
- [限制](/r2/platform/limits)
- [行为差异](/r2/platform/deviations)
