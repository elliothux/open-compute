# Guides

## Create a namespace and bind

See [Get started](/durable-objects/get-started/). The binding must include `className`.

## Call from a Worker

```ts
const id = env.COUNTER.idFromName("global");
const stub = env.COUNTER.get(id);
return stub.fetch(request);
```

`get(id, { locationHint: "weur" })` is legal, but the hint has no geographic effect.

## storage

```ts
await this.ctx.storage.put("k", { n: 1 });
const v = await this.ctx.storage.get<{ n: number }>("k");
await this.ctx.storage.transaction(async (txn) => {
  await txn.put("k", { n: (v?.n ?? 0) + 1 });
});
this.ctx.storage.sql.exec("CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, body TEXT)");
```

SQL cannot query `__open_compute_do_*` internal tables.

## Alarms and hibernation

Alarms: [alarms](/durable-objects/alarms). Hibernation: [WebSockets](/workers/runtime-apis/websockets) or [Concepts](/durable-objects/concepts/).
