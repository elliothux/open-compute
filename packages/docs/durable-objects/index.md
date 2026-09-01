# Durable Objects

Durable Objects 把计算和强一致存储绑在一个对象上。本平台上，所有对象都落在本地这一个 `workerd` 进程。location hint、jurisdiction 和全球迁移没有地理调度效果。

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

## 与 Cloudflare 相同

Worker / class API 与 [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) 相同：namespace `idFromName` / `newUniqueId` / `idFromString` / `get` / `getByName`、stub `fetch` / RPC、`state.storage` KV 与 SQL、transaction、output gate。115 个目标成员为 `supported_with_deviation`。Alarms（7 个成员）和 WebSocket hibernation（19 个成员）为 `supported`。Alarms 见 [alarms](/durable-objects/alarms)；hibernation 见本节或 [WebSockets](/workers/runtime-apis/websockets)。

```json
{
  "name": "do-app",
  "main": "src/index.ts",
  "bindings": {
    "COUNTER": { "type": "do_namespace", "id": "<do-namespace-id>", "className": "Counter" }
  }
}
```

Durable Object 必须提供 `className`：只用于核对 class 语义，不作为资源 ID 发给平台。语法见 [bindings](/workers/configuration/bindings)。

## 故意不同

**`OC-DO-001`**：Durable Objects 落在本地这一个 workerd 进程上。location hint、jurisdiction 和全球迁移没有地理调度效果。三个 `connect` 成员另外带 `OC-WKR-TCP-001` / `OC-WKR-LIMIT-001`（命名 DO `Fetcher.connect()` 走声明的 capability tunnel，不是第二条通用出网）。

全文见 [偏差](/durable-objects/platform/deviations) 和 [Compatibility](/platform/compatibility)。

## 本节

- [上手](/durable-objects/get-started/)
- [概念](/durable-objects/concepts/)
- [指南](/durable-objects/guides/)
- [示例](/durable-objects/examples/)
- [Alarms](/durable-objects/alarms)
- [限制](/durable-objects/platform/limits)
- [偏差](/durable-objects/platform/deviations)
