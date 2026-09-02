# Durable Objects

Durable Object 将计算与强一致存储绑定在同一对象上。在 open-compute 上，所有对象运行在本机的单个 `workerd` 进程中。

例如：

- 在多个客户端之间协调状态
- 每个对象独立的强一致存储
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

在 `wrangler.jsonc` 中使用 Wrangler 标准 Durable Object 字段绑定：

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "durable_objects": {
    "bindings": [{ "name": "COUNTER", "class_name": "Counter" }]
  }
}
```

class 随 Worker 上传；Durable Object migration 使用 Wrangler 标准 `migrations` 字段。语法见[绑定](/zh/workers/configuration/bindings)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| Worker / class API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | 相同：namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`、stub `fetch` / RPC、`state.storage` 的 KV 与 SQL、transaction、output gate |
| 对象位置 | 按地区调度，`locationHint` / jurisdiction / migration | 全部位于本机单个 workerd；`locationHint` / jurisdiction / migration 不产生地理效果 |
| Alarms | 提供 | 提供：`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` |
| Hibernation | 提供 | 提供 |
| 绑定 | Wrangler `durable_objects` | 标准 `name` 与 `class_name`，必须指定 `class_name` |
| `Fetcher.connect()` | 通用出站 | 使用绑定声明的连接，而非第二条通用出站通道 |

## 本节

- [上手](/zh/durable-objects/get-started/)
- [概念](/zh/durable-objects/concepts/)
- [指南](/zh/durable-objects/guides/)
- [示例](/zh/durable-objects/examples/)
- [Alarms](/zh/durable-objects/alarms)
- [限制](/zh/durable-objects/platform/limits)
- [行为差异](/zh/durable-objects/platform/deviations)
