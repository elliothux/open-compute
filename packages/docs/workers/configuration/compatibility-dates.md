# Compatibility dates

本平台只有一个生效 compatibility date，来自正式 runtime lock 的 `effective_compatibility_date`，当前为 **`2026-08-30`**。项目不能选择日期。

```sh
ocd capabilities --json
```

读取 `runtime.effective_compatibility_date`。运行中的值以该 JSON 为准。

日期的含义与 Cloudflare 对齐：它选定 workerd 在该日的可观察行为。说明见 [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 日期选定 workerd 可观察行为 | 是 | 是 |
| 每个项目设置 `compatibility_date` / `compatibilityDate` | 是 | 不允许；写入 `open-compute.json` 会作为未知字段被拒绝 |
| 更改日期的方式 | 项目配置 | 更换平台 pin（`workerd.lock.json`） |

