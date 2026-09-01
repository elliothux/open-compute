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

Bind in `open-compute.json`. Durable Object bindings require `className`:

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<do-namespace-id>", "className": "Counter" }
  }
}
```

`className` only checks class semantics in generated framework config. It is not sent as the resource id. Grammar: [bindings](/en/workers/configuration/bindings). The CLI is `oc` / `oc run` / `oc types`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | Same: namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`, stub `fetch` / RPC, `state.storage` KV and SQL, transactions, output gate |
| Placement | Geographic scheduling, `locationHint` / jurisdiction / migration | All objects on one local workerd; `locationHint` / jurisdiction / migration have no geo effect |
| Alarms | Available | 7 methods supported: `getAlarm` / `setAlarm` / `deleteAlarm` and the `alarm()` handler |
| Hibernation | Available | Supported |
| Binding | wrangler `durable_objects` | `{ type, id, className }`; `className` required |
| `Fetcher.connect()` | General outbound | Declared capability tunnel |

## Next

- [Get started](/en/durable-objects/get-started/)
- [Concepts](/en/durable-objects/concepts/)
- [Guides](/en/durable-objects/guides/)
- [Examples](/en/durable-objects/examples/)
- [Alarms](/en/durable-objects/alarms)
- [Limits](/en/durable-objects/platform/limits)
- [Behavior differences](/en/durable-objects/platform/deviations)
