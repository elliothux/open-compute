# Examples

## Read/write JSON

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const key = url.pathname.slice(1) || "default";
    if (request.method === "PUT") {
      await env.KV.put(key, await request.text(), { expirationTtl: 86400 });
      return new Response("ok");
    }
    const value = await env.KV.get(key);
    return new Response(value ?? "missing", { status: value ? 200 : 404 });
  },
} satisfies ExportedHandler<Env>;
```

## list + metadata

```ts
const page = await env.KV.list({ prefix: "cfg:", limit: 20 });
for (const key of page.keys) {
  const row = await env.KV.getWithMetadata(key.name, "json");
  console.log(key.name, row.metadata, row.value);
}
```

Config: [Get started](/en/kv/get-started/). Do not port Cloudflare geolocation or global-purge examples.
