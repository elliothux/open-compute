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

Bind an existing namespace in `open-compute.json`. Ordinary product bindings are `{ type, id, permissions? }`:

```json
{
  "name": "kv-app",
  "main": "src/index.ts",
  "bindings": {
    "KV": { "type": "kv_namespace", "id": "<kv-namespace-id>" }
  }
}
```

`id` is an existing namespace on this platform. Optional `permissions`: `{ "read": true, "write": true }`. Binding grammar: [Workers configuration · bindings](/en/workers/configuration/bindings). The CLI is `oc` / `oc run` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker API | [Cloudflare KV API](https://developers.cloudflare.com/kv/api/) | Same: `put` / `get` / `getWithMetadata` / `list` / `delete`, text / json / arrayBuffer / stream, metadata, TTL, bulk get, list cursor |
| Replication | Global edge | Single-node SQLite on the node running ocd |
| `cacheTtl` | Colo cache | Parameter accepted; no colo cache |
| Jurisdictions | Available | Not provided |
| REST / `client.v4` | Available | Not provided; use the Worker binding |

## Next

- [Get started](/en/kv/get-started/)
- [Concepts](/en/kv/concepts/)
- [Guides](/en/kv/guides/)
- [Examples](/en/kv/examples/)
- [Limits](/en/kv/platform/limits)
- [Behavior differences](/en/kv/platform/deviations)
