# Guides

Create a definition, bind `className` in `open-compute.json`, then create instances with `oc`. Committed steps are skipped on replay.

## Create and bind

See [Get started](/en/workflows/get-started/). The binding must include `className`. `schedules` is optional.

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
