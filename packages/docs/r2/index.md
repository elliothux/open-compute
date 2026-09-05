# R2

R2 is object storage that lets you store and retrieve unstructured data from a Worker. The Worker binding API matches Cloudflare. Object bytes are held by the platform-wide Local or S3 backend selected by the operator.

For example, you can use R2 for:

- Storage for unstructured objects
- Serving files from a Worker
- Multipart uploads on either supported backend

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

Bind an existing logical bucket with Wrangler's standard R2 field:

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "r2_buckets": [{ "binding": "BUCKET", "bucket_name": "files" }]
}
```

`bucket_name` names an existing logical bucket in the account. Binding grammar: [bindings](/workers/configuration/bindings). Use pinned Wrangler or the official SDK for bucket and object operations.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `head` / `get` / `put` / `delete` / `list`, conditional writes, checksums, multipart, HTTP metadata |
| Object bytes | Cloudflare R2 storage | Configured Local or S3 authority on one node |
| Global placement | Available | Not provided |
| r2.dev public product | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| REST / `client/v4` | Available | Compatible account-scoped bucket and object operations |

## Next

- [Get started](/r2/get-started/)
- [Concepts](/r2/concepts/)
- [Guides](/r2/guides/)
- [Examples](/r2/examples/)
- [Limits](/r2/platform/limits)
- [Behavior differences](/r2/platform/deviations)
