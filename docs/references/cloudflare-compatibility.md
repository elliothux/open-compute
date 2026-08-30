# Cloudflare Workers 兼容矩阵

本页是 [`share/cloudflare-capabilities.json`](../../share/cloudflare-capabilities.json) 与
[`test/conformance/catalog.json`](../../test/conformance/catalog.json) 的人类可读索引，不建立第二份
能力真值。方法、状态、deviation、source identity 或 Gate 变化时，机器可读文件和本页必须一起
更新；`platformd capabilities --json` 直接读取同一份 capability authority。

本页只报告当前实现，不定义最终产品范围。全量目标见
[`Cloudflare Worker Runtime 全量兼容目标`](../cloudflare-runtime-compatibility.md)：Workers runtime、
Durable Objects、Queues、Workflows、R2、D1、KV 的 upstream stable tenant API 必须完整；本页中的
方法子集或 `unsupported` 记录不能被解释为永久边界，目标成员的当前缺口在迁移时应归类为 `blocked`。

固定契约输入见 [`baseline.json`](../../test/conformance/baseline.json)。当前测试日期为
`2026-08-26`，允许 flags 为 `nodejs_compat`、`rpc`、`assets_navigation_has_no_effect` 与
`assets_navigation_prefers_asset_serving`；每个组合仍须经过部署 parser、动态 loader 和对应产品
Gate。Cloudflare 官方文档、workers-types、workers-sdk 与 workerd 的精确
revision/digest 均记录在 baseline/catalog，不以 live URL 或浮动 `latest` 代替。

这是旧的多 date/flag 实现快照，不是新的兼容目标。目标模型不接受 tenant compatibility date/flags，
而是在 coordinated dependency update 时固定唯一 `effectiveCompatibilityDate`、所需内部 flags、匹配
workerd 和 upstream types；完成迁移前不得把当前 baseline 描述成 single-latest。

## 当前支持面

| 产品 | 状态 | 方法/行为 | deviation |
| --- | --- | --- | --- |
| Workers runtime | `supported` | fetch、RPC、Streams、基础 WebSocket、HTTP(S) outbound | — |
| Deployments | `supported_with_deviation` | create/validate/stage/ready、promote/rollback/route/delete、vars/secrets | `OC-DEPLOY-001` |
| Static Assets | `supported_with_deviation` | binding fetch、默认 routing、HTTP、不可变 publish/rollback | `OC-ASSETS-001` |
| Service Binding | `supported_with_deviation` | default/named fetch、RPC、self、event-source、pin/lifecycle | `OC-SERVICE-001` |
| KV | `supported_with_deviation` | get/getWithMetadata/put/delete/list | `OC-KV-001` |
| R2 | `supported_with_deviation` | head/get/put/delete/list | `OC-R2-001` |
| D1 | `supported_with_deviation` | prepare/batch/exec/withSession、run/all/first/raw | `OC-D1-001` |
| Durable Objects | `supported_with_deviation` | namespace/ID、fetch/RPC、storage get/put/delete/list/transaction、基础 WebSocket | `OC-DO-001`、`OC-WS-001` |
| DO Alarms | `supported` | getAlarm/setAlarm/deleteAlarm/alarm | — |
| Queues | `supported_with_deviation` | send/sendBatch/metrics、queue、ack/retry/ackAll/retryAll | `OC-QUEUE-001` |
| Cron | `supported_with_deviation` | scheduled/noRetry | `OC-CRON-001` |
| Workflows | `supported_with_deviation` | create/get/id/status、step do/sleep/sleepUntil/waitForEvent、event/lifecycle | `OC-WORKFLOW-001`、`OC-WORKFLOW-002` |
| Workers Cache | `supported_with_deviation` | response fetch/purge | `OC-CACHE-001`、`OC-CACHE-002` |
| Cache API | `supported_with_deviation` | default/open/put/match/delete | `OC-CACHE-001`、`OC-CACHE-002` |
| Images | `supported_with_deviation` | input/info/transform/draw/output/response/contentType/image | `OC-IMAGES-001` |
| Version Metadata | `supported` | id/tag/timestamp | — |

deviation 的规范文本和边界测试归 [`p1-deviations.md`](p1-deviations.md) 所有。单节点没有 edge
placement、跨地域复制或 Cloudflare 管理/计费面；这些差异不放宽安全边界、事务原子性、不可变
deployment、明确错误、本地 crash recovery 或账户隔离。

## 当前未支持与非目标

当前 capability authority 把 WebSocket hibernation、Analytics Engine、AI、Browser Rendering、
Vectorize、Hyperdrive、mTLS、Rate Limiting 与 Workers for Platforms 记为 `unsupported`，且部署配置
在 authority 边界 fail closed。新的全量目标要求 Durable Objects hibernatable WebSocket，因此该项
是待迁移的 `blocked` 缺口，不是允许的永久 deviation；其余列举产品不属于本次七项产品目标。

上游完整 `@cloudflare/workers-types` 可以包含这些非目标产品的 type name。边界由 generated `Env`、
runtime availability、capability/catalog 和配置拒绝共同建立；不得再通过手写或裁剪类型 package
隐藏未实现能力。

## 证据与结论

`./test/gate.py p3-contract` 验证 capability/catalog/type/config/deviation/case/source 双射；
`./test/gate.py p3` 执行本地 L0–L3/L6 contract/product 目标。Service Binding 的产品证据还包括
Queue、Cron、Durable Object、Workflow 四类真实事件源调用，以及真实 workerd SIGKILL 后在途
handle/pin 清理。

2026-08-30 在显式外部写入授权下运行了一项 Cache API portable fixture：真实 Cloudflare 与
open-compute 都依次观察到 `/reset` 的 `200 {"reset":true}`，以及两次 `/probe` 的
`200 {"cache":"MISS","body":"portable-cache-v1"}`、
`200 {"cache":"HIT","body":"portable-cache-v1"}`。runner 随机创建单个 workers.dev Worker，
没有 route、binding 或共享资源，随后按精确名称删除；二次只读查询确认不存在。

该结果只 qualification 当前 catalog 中这一项 Cache API contract，不代表所有 Workers、Durable
Objects、Queues、Workflows、R2、D1、KV 或 Static Assets/Service Binding 高风险行为已经对照。
[全量兼容目标](../cloudflare-runtime-compatibility.md)要求扩展 `CF-TEST-03`，当前仍为
active/blocked，不能声称已与 Cloudflare 全量实测一致。第三方应用没有固定 application manifest
或未显式选择，Application verdict 仍是“未评估”，不会改变 Platform contract。
