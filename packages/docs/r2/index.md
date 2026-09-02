# R2

R2 is object storage that lets you store and retrieve unstructured data from a Worker. The Worker binding API matches Cloudflare. Object bytes are held by the S3-compatible provider you configured.

For example, you can use R2 for:

- Storage for unstructured objects
- Serving files from a Worker
- Multipart uploads to the configured S3 provider

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

Bind an existing logical bucket in `open-compute.json`. Ordinary product bindings are `{ type, id, permissions? }`:

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<r2-bucket-id>" }
  }
}
```

`id` is an existing logical bucket on this platform. Binding grammar: [bindings](/workers/configuration/bindings). The CLI is `oc` / `oc run` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/) | Same: `head` / `get` / `put` / `delete` / `list`, conditional writes, checksums, multipart, HTTP metadata |
| Object bytes | Cloudflare R2 storage | Configured S3-compatible provider |
| Global placement | Available | Not provided |
| r2.dev public product | Available | Not provided |
| Jurisdictional restrictions | Available | Not provided |
| REST / `client.v4` | Available | Not provided; use the Worker binding or the provider S3 API |

## Next

- [Get started](/r2/get-started/)
- [Concepts](/r2/concepts/)
- [Guides](/r2/guides/)
- [Examples](/r2/examples/)
- [Limits](/r2/platform/limits)
- [Behavior differences](/r2/platform/deviations)
