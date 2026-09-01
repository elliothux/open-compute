# 平台

open-compute 在单节点上运行已声明的 Cloudflare Workers 编程模型。Worker 侧 API 与 Cloudflare 文档一致；存储、调度与部署落在本机。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。JSON 顶层：`schema_version`、`release`、`runtime`、`products`、`limits`。

## 兼容性

| 方面 | 说明 |
| --- | --- |
| Worker API | Workers、KV、D1、R2、Durable Objects、Queues、Workflows、Cache、Images 的 Worker 侧符号与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致。签名索引见 [API 参考](/platform/reference/api/)。 |
| 拓扑 | 单节点：一个 `ocd`、一个固定版本的 `workerd`、本机权威存储。没有全球边缘、dashboard、计费，也没有 Cloudflare REST v4 / `client.v4`。 |
| 项目配置 | `open-compute.json`，不是 `wrangler.jsonc`。未知字段会拒绝。 |
| 限制 | 以运行中二进制的 `ocd capabilities --json` 为准。 |
| 行为差异 | 见[行为差异](/platform/deviations)。 |
| 未提供的产品 | 见[不支持](/platform/unsupported)。 |

## 本节

- [兼容性](/platform/compatibility) — 产品、Worker API、单节点拓扑
- [行为差异](/platform/deviations) — TCP、限额、存储拓扑与其它行为
- [限制](/platform/limits) — 运行中 `capabilities.limits`
- [不支持](/platform/unsupported) — 未提供的 Cloudflare 产品
- [API 参考](/platform/reference/api/) — 生成成员索引入口
