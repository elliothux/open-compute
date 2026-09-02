# KV

Workers KV is a data storage that allows you to store and retrieve key-value data from a Worker. On this platform, each namespace is a SQLite database on the node running ocd.

For example, you can use Workers KV for:

- Caching API responses
- Storing user configurations / preferences
- Storing user authentication details

```ts
export default {
  async fetch(request, env, ctx): Promise<Response> {
    // write a key-value pair
    await env.KV.put("KEY", "VALUE");

    // read a key-value pair
    const value = await env.KV.get("KEY");

    // list all key-value pairs
    const allKeys = await env.KV.list();

    // delete a key-value pair
    await env.KV.delete("KEY");

    return Response.json({ value, allKeys });
  },
} satisfies ExportedHandler<{ KV: KVNamespace }>;
```

Bind an existing namespace with Wrangler's standard KV field:

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "kv_namespaces": [{ "binding": "KV", "id": "<kv-namespace-id>" }]
}
```

`id` is an existing namespace in the account. Binding grammar: [Workers configuration · bindings](/workers/configuration/bindings). Use pinned Wrangler or the official SDK for namespace and value operations.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [Cloudflare KV API](https://developers.cloudflare.com/kv/api/) | Same: `put` / `get` / `getWithMetadata` / `list` / `delete`, text / json / arrayBuffer / stream, metadata, TTL, bulk get, list cursor |
| Replication | Global edge | Single-node SQLite on the node running ocd |
| `cacheTtl` | Colo cache | Parameter accepted; no colo cache |
| Jurisdictions | Available | Not provided |
| REST / `client/v4` | Available | Compatible account-scoped namespace and value operations |

## Next

- [Get started](/kv/get-started/)
- [Concepts](/kv/concepts/)
- [Guides](/kv/guides/)
- [Examples](/kv/examples/)
- [Limits](/kv/platform/limits)
- [Behavior differences](/kv/platform/deviations)
