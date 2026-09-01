# 指南

创建 definition，在 `open-compute.json` 中绑定 `className`，再用 `oc` 创建实例。已提交的 step 在 replay 时跳过。

## 创建并绑定

见[上手](/workflows/get-started/)。binding 必须有 `className`。可选 `schedules`。

## 创建实例

```ts
const instance = await env.FLOW.create({ params: { hello: "world" } });
const same = await env.FLOW.get(instance.id);
await same.status();
```

`createBatch` / `deleteBatch` 每批 1–100 条。

## step.do

```ts
await step.do("charge", async () => chargeCustomer());
await step.sleep("wait", "1 minute");
```

已提交的 step 在 replay 时跳过。未提交的 callback 可能重复，直到结果写入 SQLite。外部副作用不随 snapshot 回滚。
