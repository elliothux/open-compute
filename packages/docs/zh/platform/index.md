# 平台

open-compute 在单机上提供与 Cloudflare Workers 文档一致的 Worker API。存储、调度与部署均在本机完成。

```sh
ocd --config /etc/open-compute/config.toml capabilities --json
```

省略 `--config` 时，`limits` 来自内嵌默认配置。JSON 顶层字段：`schema_version`、`release`、`runtime`、`products`、`limits`。

## 兼容性

| 方面 | 说明 |
| --- | --- |
| Worker API | Workers、KV、D1、R2、Durable Objects、Queues、Workflows、Cache、Images 的 Worker 用法与 [Workers runtime APIs](https://developers.cloudflare.com/workers/runtime-apis/) 一致。签名索引见 [API 参考](/zh/platform/reference/api/)。 |
| 数据与进程 | 单机：一个 `ocd`、一个固定版本的 `workerd`、本机 authority storage。管理面使用兼容 `/client/v4` API 与 SDK 驱动的 Dashboard；不提供全球边缘网络和计费。 |
| 项目配置 | 标准 `wrangler.jsonc`，由固定 `wrangler@4.127.1` 解析；不支持的 server 能力 fail closed。 |
| 限制 | 以运行中的 `ocd capabilities --json` 为准。 |
| 行为差异 | 见[行为差异](/zh/platform/deviations)。 |
| 未提供的产品 | 见[不支持](/zh/platform/unsupported)。 |

## 本节

- [兼容性](/zh/platform/compatibility) — 产品、Worker API 与数据位置
- [行为差异](/zh/platform/deviations) — 出站、限额与存储位置
- [限制](/zh/platform/limits) — 运行中的 `capabilities.limits`
- [不支持](/zh/platform/unsupported) — 未提供的 Cloudflare 产品
- [API 参考](/zh/platform/reference/api/) — 按产品统计的 API 成员入口
