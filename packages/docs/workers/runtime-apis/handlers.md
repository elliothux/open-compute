# Handlers

Methods on the module Worker's exported object.

```ts
export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    return new Response("ok");
  },
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {},
  async queue(batch: MessageBatch, env: Env, ctx: ExecutionContext): Promise<void> {},
} satisfies ExportedHandler<Env>;
```

Durable Object `alarm()` lives on the DO class, not the default export. See [DO Alarms](https://developers.cloudflare.com/durable-objects/api/alarms/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `fetch`, `scheduled`, and `queue` arguments and return values | Yes — [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/) | Yes |
| `ctx.waitUntil`, `ctx.passThroughOnException` | Yes | Follow workerd |
| Email handler / Tail handler | Yes | Not provided |
| Cron trigger | Hosted Cron | UTC five-field; misfire projects at most the latest slot in grace — [Cron Triggers](/workers/configuration/cron-triggers) |
| Queue delivery | Global queue semantics | Single-node at-least-once, not global FIFO |
| `triggers.crons` / queue-consumer array in the project file | Wrangler | Not allowed; platform deployment metadata accepts `crons` and `queue_consumers` |

