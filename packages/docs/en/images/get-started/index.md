# Get started

There is no resource to create: Images is a deployment-scoped binding and does not reference a namespace id. `ocd` must be ready.

## 1. Declare the binding

```json
{
  "name": "img-app",
  "main": "src/index.ts",
  "images": { "binding": "IMAGES" }
}
```

Do not put it in `bindings`. Do not supply a Cloudflare Images account id.

```sh
bun run oc types --config open-compute.json
```

## 2. Worker

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    if (!request.body) return new Response("empty", { status: 400 });
    const info = await env.IMAGES.info(request.body);
    // info.format is jpeg | png | webp
    const out = await env.IMAGES
      .input(request.body)
      .transform({ width: 320, fit: "contain" })
      .output({ format: "image/webp", quality: 80 });
    return out.response();
  },
} satisfies ExportedHandler<Env>;
```

Input must be request-body bytes. There is no `https://imagedelivery.net/...` URL transform and no upload/signing.

## 3. Run

```sh
bun run oc run --config open-compute.json --ocd <path-to-ocd>
```

The CLI is `oc`, not Wrangler. Next: [Concepts](/en/images/concepts/).
