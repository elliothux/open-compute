# Behavior differences

The Durable Objects Worker / class API matches Cloudflare. Every object lives on the single local workerd.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | Same: namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`, stub `fetch` / RPC, `state.storage` KV and SQL, transactions, output gate |
| Placement | Geographic scheduling, `locationHint` / jurisdiction / migration | All objects on one local workerd; `locationHint` / jurisdiction / migration have no geo effect |
| Alarms | Available | 7 methods supported |
| Hibernation | Available | Supported |
| Binding | Wrangler `durable_objects` | Standard `name`, `class_name`, and optional `script_name` |
| `Fetcher.connect()` | General outbound | Declared capability tunnel |

See [Compatibility](/platform/compatibility).
