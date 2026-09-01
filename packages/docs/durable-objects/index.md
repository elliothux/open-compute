# Durable Objects

Durable Objects 把计算和强一致存储绑在一个对象上。本平台上，所有对象都落在本地这一个 `workerd` 进程。

例如：

- 在多个客户端之间协调状态
- 每个对象的强一致存储
- Alarms 与 WebSocket hibernation

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

在 `open-compute.json` 中绑定。Durable Object 必须提供 `className`：

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<do-namespace-id>", "className": "Counter" }
  }
}
```

`className` 只用于核对 class 语义，不作为资源 ID 发给平台。语法见 [bindings](/workers/configuration/bindings)。CLI 为 `oc` / `oc run` / `oc types`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | 相同：namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`、stub `fetch` / RPC、`state.storage` KV 与 SQL、transaction、output gate |
| 放置 | 地理调度，`locationHint` / jurisdiction / migration | 全部对象在本地这一个 workerd；`locationHint` / jurisdiction / migration 无地理效果 |
| Alarms | 提供 | 支持 7 个方法：`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` handler |
| Hibernation | 提供 | 支持 |
| Binding | wrangler `durable_objects` | `{ type, id, className }`；`className` 必填 |
| `Fetcher.connect()` | 通用出网 | 声明的 capability tunnel |

## 本节

- [上手](/durable-objects/get-started/)
- [概念](/durable-objects/concepts/)
- [指南](/durable-objects/guides/)
- [示例](/durable-objects/examples/)
- [Alarms](/durable-objects/alarms)
- [限制](/durable-objects/platform/limits)
- [行为差异](/durable-objects/platform/deviations)
