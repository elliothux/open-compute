# KV

Workers KV is key-value storage bound onto Worker `env`. On this platform, each namespace is a SQLite database on this one machine. There is no global edge cache and no cross-node replication.

```ts
export default {
  async fetch(request, env, ctx): Promise<Response> {
    await env.KV.put("KEY", "VALUE");
    const value = await env.KV.get("KEY");
    const allKeys = await env.KV.list();
    await env.KV.delete("KEY");
    return Response.json({ value, allKeys });
  },
} satisfies ExportedHandler<{ KV: KVNamespace }>;
```

## Same as Cloudflare

The Worker binding API is the [Cloudflare KV API](https://developers.cloudflare.com/kv/api/): `put` / `get` / `getWithMetadata` / `list` / `delete`, plus text / json / arrayBuffer / stream, metadata, TTL, bulk get, and list cursors. 52 target members are `supported_with_deviation`.

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` is an already-existing namespace on the platform. Binding grammar: [Workers configuration · bindings](/en/workers/configuration/bindings). Do not copy Cloudflare REST or Wrangler KV subcommands from this page.

## Intentional differences

**`OC-KV-001`**: KV is single-node SQLite authority. It does not claim Cloudflare global replication or edge-cache propagation timing. `cacheTtl` is accepted for parameter compatibility and does not create a colo cache. There is no jurisdiction product and no `api.cloudflare.com/client/v4`.

Full text: [Deviations](/en/kv/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/kv/get-started/)
- [Concepts](/en/kv/concepts/)
- [Guides](/en/kv/guides/)
- [Examples](/en/kv/examples/)
- [Limits](/en/kv/platform/limits)
- [Deviations](/en/kv/platform/deviations)
