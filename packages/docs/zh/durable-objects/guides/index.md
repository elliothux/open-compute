# 指南

## 创建 namespace 并绑定

见[上手](/zh/durable-objects/get-started/)。binding 必须有 `className`。

## 从 Worker 调用

```ts
const id = env.COUNTER.idFromName("global");
const stub = env.COUNTER.get(id);
return stub.fetch(request);
```

`get(id, { locationHint: "weur" })` 合法，但 hint 没有地理效果。

## storage

```ts
await this.ctx.storage.put("k", { n: 1 });
const v = await this.ctx.storage.get<{ n: number }>("k");
await this.ctx.storage.transaction(async (txn) => {
  await txn.put("k", { n: (v?.n ?? 0) + 1 });
});
this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, body TEXT)");
```

SQL 不能查询 `__open_compute_do_*` 内部表。

## Alarms 与 hibernation

Alarms：[alarms](/zh/durable-objects/alarms)。Hibernation：[WebSockets](/zh/workers/runtime-apis/websockets) 或[概念](/zh/durable-objects/concepts/)。
