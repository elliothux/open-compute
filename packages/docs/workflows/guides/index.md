# Guides

Create a definition, bind `class_name` in `wrangler.jsonc`, then create instances through the official SDK. Committed steps are skipped on replay.

## Create and bind

See [Get started](/workflows/get-started/). The binding must include `class_name`. `schedules` is optional.

## Create an instance

```ts
const instance = await env.FLOW.create({ params: { hello: "world" } });
const same = await env.FLOW.get(instance.id);
await same.status();
```

`createBatch` / `deleteBatch` are 1–100 per call.

## step.do

```ts
await step.do("charge", async () => chargeCustomer());
await step.sleep("wait", "1 minute");
```

Committed steps are skipped on replay. Uncommitted callbacks may repeat until the result is written to SQLite. External side effects do not roll back with the snapshot.
