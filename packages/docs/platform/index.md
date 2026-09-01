# 平台

open-compute 的平台页描述**这台机器上的契约**：开了什么、故意做成什么样、数字上限是多少。不要用产品名字推导 Cloudflare 全量行为。权威是正在运行的 `ocd capabilities --json`。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。JSON 顶层：`schema_version`、`release`、`runtime`、`products`、`limits`。

## 与 Cloudflare 相同

Worker / KV / D1 / R2 / Durable Objects / Queues / Workflows / Cache / Images 的 **Worker 侧符号**跟 Cloudflare 文档同一套表面。成员签名不要在本站手抄：去 [API 参考](/platform/reference/api/) 和 Cloudflare 原文。

## 故意不同

没有全球边缘、没有 dashboard、没有计费、没有 Cloudflare REST v4 / `client.v4`。项目文件是 `open-compute.json`，不是 `wrangler.jsonc`。已登记差异只通过 `OC-*` ID 声明，见[偏差](/platform/deviations)。非目标产品见[不支持](/platform/unsupported)。

## 本节

- [兼容性](/platform/compatibility) — 契约、状态语义、成员库存
- [偏差](/platform/deviations) — 15 个已登记 `OC-*` ID
- [限制](/platform/limits) — 运行中 `capabilities.limits`
- [不支持](/platform/unsupported) — `non_target` 产品
- [API 参考](/platform/reference/api/) — 生成成员索引入口
