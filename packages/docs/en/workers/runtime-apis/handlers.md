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

## Same as Cloudflare

`fetch`, `scheduled`, and `queue` arguments and return values match [Handlers](https://developers.cloudflare.com/workers/runtime-apis/handlers/). `ctx.waitUntil` and `ctx.passThroughOnException` follow workerd. There is no Email handler and no Tail handler product.

## Intentional delta

Cron trigger semantics: [`OC-CRON-001`](/en/workers/configuration/cron-triggers). Queue delivery is single-node at-least-once (`OC-QUEUE-001`), not global FIFO. `open-compute.json` currently has no `triggers.crons` or queue-consumer array; unknown fields fail. Platform deployment metadata accepts `crons` and `queue_consumers`.
