# Alarms

Alarms are timers on a Durable Object. Cloudflare documents them under the [DO API](https://developers.cloudflare.com/durable-objects/api/); this site keeps them under the Durable Objects product. 7 target members are `supported` (no alarm deviation ID). The object still lives on this one workerd (`OC-DO-001` describes placement; it does not strip alarm methods).

```ts
export class Snooze {
  constructor(private readonly ctx: DurableObjectState) {}
  async fetch(): Promise<Response> {
    await this.ctx.storage.setAlarm(Date.now() + 10_000);
    return new Response("armed");
  }
  async alarm(): Promise<void> {
    await this.ctx.storage.put("fired", true);
  }
}
```

`getAlarm` / `setAlarm` / `deleteAlarm` and the `alarm()` handler match Cloudflare. There is no cross-region wake-up and no Cloudflare dashboard alarm observability.
