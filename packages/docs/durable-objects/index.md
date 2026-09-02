# Durable Objects

Durable Objects bind compute and strongly consistent storage to one object. On this platform every object lives on the single local `workerd` process.

For example, you can use Durable Objects for:

- Coordinating state among multiple clients
- Strongly consistent per-object storage
- Alarms and WebSocket hibernation

```ts
export class Counter {
  constructor(private readonly ctx: DurableObjectState, private readonly env: Env) {}
  async fetch(request: Request): Promise<Response> {
    const n = ((await this.ctx.storage.get<number>("n")) ?? 0) + 1;
    await this.ctx.storage.put("n", n);
    return Response.json({ n });
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const id = env.COUNTER.idFromName("global");
    return env.COUNTER.get(id).fetch(request);
  },
} satisfies ExportedHandler<{ COUNTER: DurableObjectNamespace }>;
```

Bind in `wrangler.jsonc` with Wrangler's standard Durable Object field:

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "durable_objects": {
    "bindings": [{ "name": "COUNTER", "class_name": "Counter" }]
  }
}
```

The class is part of the uploaded Worker; Durable Object migrations follow Wrangler's standard `migrations` field. Grammar: [bindings](/workers/configuration/bindings).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | Same: namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`, stub `fetch` / RPC, `state.storage` KV and SQL, transactions, output gate |
| Placement | Geographic scheduling, `locationHint` / jurisdiction / migration | All objects on one local workerd; `locationHint` / jurisdiction / migration have no geo effect |
| Alarms | Available | 7 methods supported: `getAlarm` / `setAlarm` / `deleteAlarm` and the `alarm()` handler |
| Hibernation | Available | Supported |
| Binding | Wrangler `durable_objects` | Standard `name` and `class_name`; `class_name` required |
| `Fetcher.connect()` | General outbound | Declared capability tunnel |

## Next

- [Get started](/durable-objects/get-started/)
- [Concepts](/durable-objects/concepts/)
- [Guides](/durable-objects/guides/)
- [Examples](/durable-objects/examples/)
- [Alarms](/durable-objects/alarms)
- [Limits](/durable-objects/platform/limits)
- [Behavior differences](/durable-objects/platform/deviations)
