# R2

R2 is object storage bound onto Worker `env`. The Worker binding API matches Cloudflare; object bytes are held by the S3-compatible provider you configured. There is no Cloudflare global placement.

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

## Same as Cloudflare

The Worker binding is the [R2 Workers API](https://developers.cloudflare.com/r2/api/workers/workers-api-reference/): `head` / `get` / `put` / `delete` / `list`, conditional writes, checksums, multipart, HTTP metadata. 110 target members are `supported_with_deviation`. Object bytes can also be reached through the configured S3-compatible API (the provider's own SDK). That is the storage protocol, not a second Cloudflare REST.

```json
{
  "name": "r2-app",
  "main": "src/index.ts",
  "bindings": {
    "BUCKET": { "type": "r2_bucket", "id": "<r2-bucket-id>" }
  }
}
```

`id` is an already-existing logical bucket on the platform. Binding grammar: [bindings](/en/workers/configuration/bindings). Do not copy `client.v4` from this page.

## Intentional differences

**`OC-R2-001`**: R2 object bytes are held by the configured S3-compatible provider. The platform does not claim Cloudflare global placement or replication. No smart proximity, no Cloudflare-hosted public r2.dev product.

Full text: [Deviations](/en/r2/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/r2/get-started/)
- [Concepts](/en/r2/concepts/)
- [Guides](/en/r2/guides/)
- [Examples](/en/r2/examples/)
- [Limits](/en/r2/platform/limits)
- [Deviations](/en/r2/platform/deviations)
