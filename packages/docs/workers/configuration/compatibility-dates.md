# Compatibility dates

本平台只有一个生效 compatibility date，来自正式 runtime lock 的 `effective_compatibility_date`，当前是 **`2026-08-30`**。项目不能选日期。

```sh
ocd capabilities --json
```

读 `runtime.effective_compatibility_date`。不要把本页抄成运行中的值；以这台机器上的 JSON 为准。

## 与 Cloudflare 相同

日期的**含义**与 Cloudflare 相同：它选定 workerd 在该日的可观察行为。说明见 [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)。

## 故意不同

没有 per-project `compatibility_date` / `compatibilityDate`。写进 `open-compute.json` 会当未知字段拒绝。换日期等于换平台 pin（`workerd.lock.json`），不是开发者配置项。
