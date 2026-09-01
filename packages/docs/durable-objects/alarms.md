# Alarms

Alarms 是 Durable Object 上的定时器。Cloudflare 把它们放在 [DO API](https://developers.cloudflare.com/durable-objects/api/) 下；本站同样挂在 Durable Objects 产品下。7 个目标成员为 `supported`（没有 alarm 偏差 ID）。对象仍在这一台 workerd 上（`OC-DO-001` 描述放置，不削弱 alarm 方法）。

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

`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` handler 与 Cloudflare 相同。没有跨区域唤醒，也没有 Cloudflare dashboard 上的 alarm 观察面。
