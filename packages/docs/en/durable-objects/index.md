# Durable Objects

Durable Objects bind compute and strongly consistent storage to one object. On this platform every object lives on the single local `workerd` process. Location hints, jurisdiction, and global migration have no geographic scheduling effect.

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

## Same as Cloudflare

The Worker / class API is the [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/): namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`, stub `fetch` / RPC, `state.storage` KV and SQL, transactions, output gate. 115 target members are `supported_with_deviation`. Alarms (7 members) and WebSocket hibernation (19 members) are `supported`. Alarms: [alarms](/en/durable-objects/alarms). Hibernation: this section or [WebSockets](/en/workers/runtime-apis/websockets).

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<do-namespace-id>", "className": "Counter" }
  }
}
```

Durable Object bindings must include `className`. It only checks class semantics in generated framework config; it is not sent as the resource id. Grammar: [bindings](/en/workers/configuration/bindings).

## Intentional differences

**`OC-DO-001`**: Durable Objects are placed on the single local workerd process. Location hints, jurisdiction, and global migration have no geographic scheduling effect. Three `connect` members also carry `OC-WKR-TCP-001` / `OC-WKR-LIMIT-001` (named DO `Fetcher.connect()` uses a declared capability tunnel, not a second general outbound).

Full text: [Deviations](/en/durable-objects/platform/deviations) and [Compatibility](/en/platform/compatibility).

## In this section

- [Get started](/en/durable-objects/get-started/)
- [Concepts](/en/durable-objects/concepts/)
- [Guides](/en/durable-objects/guides/)
- [Examples](/en/durable-objects/examples/)
- [Alarms](/en/durable-objects/alarms)
- [Limits](/en/durable-objects/platform/limits)
- [Deviations](/en/durable-objects/platform/deviations)
