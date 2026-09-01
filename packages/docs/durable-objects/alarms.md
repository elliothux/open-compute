# Alarms

Alarms 是 Durable Object 上的定时器。Cloudflare 把它们放在 [DO API](https://developers.cloudflare.com/durable-objects/api/) 下；本站同样位于 Durable Objects 产品下。`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` handler 均支持。对象仍在该节点的这一个 workerd 上。

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

`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` handler 与 Cloudflare 对齐。不提供跨区域唤醒，也不提供 Cloudflare dashboard 上的 alarm 观察面。
