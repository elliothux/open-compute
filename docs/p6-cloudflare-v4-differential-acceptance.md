# P6 Cloudflare v4 与 Wrangler 远端差分验收

状态：待外部账号凭证与产品权限

日期：2026-09-03

P6 本地实现与固定客户端验证完成后，本文只追踪尚未取得的新一轮 Cloudflare 托管端差分证据。它不把本地
`/client/v4`、固定 Wrangler、官方 SDK、真实 SQLite/S3 或 stock workerd 的通过结果替代为远端证据，也不重新打开
已经完成的 P6 核心实现。

## 未完成原因

当前验收环境没有 `CLOUDFLARE_API_TOKEN`、`CLOUDFLARE_ACCOUNT_ID` 或已确认具备 P6 产品权限的账号配置，因而
不能执行会创建 Cloudflare Worker、KV namespace、D1 database、R2 bucket、Queue、Workflow、Vectorize index 或
AI Search resource 的远端 mutation。缺少凭证是 qualification gap，不证明实现不兼容，也不能写成 PASS。

## 固定输入

- Wrangler：`4.127.1`，完整性与 package/config/CLI SHA-256 见
  [`cloudflare-openapi.lock.json`](../openapi/upstream/cloudflare-openapi.lock.json)；
- Cloudflare TypeScript SDK：`7.1.0`，revision 与哈希见同一 lock；
- Cloudflare OpenAPI：revision `b8687f42e28fbfcb296a350f7dbf16349ea900af`，固定 snapshot SHA-256
  `2ffedbbf8b25361a3be2062b7793946e7b9efc0e48b462da68f3195f12ab052b`；
- workerd：`v1.20260830.1`，revision `e9dda5963aba7ee4323960db795690ec78fec118`；
- compatibility date：`2026-08-30`；
- 已有 portable runtime/product fixture 与归一化规则：
  [`test/conformance/differential.ts`](../test/conformance/differential.ts)。该 runner 的范围不自动扩成 P6
  management、SDK 或 Assets qualification。

任一固定输入变化都需要先更新 pin、trace 与本地合同检查；不能把旧报告外推到新版本。

## 安全前置条件

执行远端差分前必须同时满足：

1. 用户明确授权本次 Cloudflare 外部写入；
2. token 和 account 通过固定 Wrangler 的只读 identity/preflight，并具备所选产品权限；
3. runner 输出本次唯一 `oc-p34-*` 前缀、精确资源数和清理计划后才开始 mutation；
4. 每个 provider 的同名资源均先证明 absent，任何冲突立即停止，绝不接管或覆盖已有资源；
5. cleanup 只按本次 ownership journal 中的精确 name/ID 删除，不使用 `force`、模糊匹配或账号级清理；
6. differential 自身的失败证据保存在 `.temp/gate-run/<oc-p34-run>/failed/`，且 token、secret、cookie、
   signed token 和对象内容均已清洗；外层 Gate 若失败，仍按 Gate runner 自己的失败目录保留整轮报告。

## Runner 范围

现有 `test/conformance/differential.ts` 是 P3.4 延续下来的 portable runner，已具备 Workers、Cache API、KV、
D1、R2、Durable Objects、Queues 与 Workflows 的同源 fixture、精确 ownership journal 和 cleanup。它可以在
同一冻结 revision 上重跑这些 portable 行为，但不能据此声称下列 P6 管理面已经取得托管端证据。

P6 仍需新增或扩展经过 safety review 的 hosted runner，覆盖：

- 固定 Wrangler 的 account discovery、Worker/Version/Deployment、secret 与资源管理命令；
- 官方 `cloudflare@7.1.0` typed client 的同客户端 envelope、分页、raw bytes 与错误形状；
- multipart metadata 与 Static Assets 三段上传、回读和精确清理。

这些 runner 尚未在当前无 credentials 环境中完成远端 preflight 或执行。已有 P6 本地真实 `ocd` Gate 证明的
是本地固定客户端合同，不替代上述 hosted runner，也不应被混入已有 portable runner 的 PASS 记录。

## 待取得证据

- 由新增 P6 runner 取得固定 Wrangler 对 account discovery、Worker
  deploy/Versions/Deployments/rollback/secrets、KV、D1、R2、Vectorize、AI Search、Queues 与 Workflows
  声明子集的 method/path/query/header/content-type/exit-code trace；
- 由现有 portable runner 在同一冻结 revision 上取得 Workers、KV、D1、R2、Durable Objects、Queues 与
  Workflows fixture 的两端可观察结果；
- 由新增 SDK runner 取得 P6 official subset 的 response envelope、分页、raw bytes、错误 code/type 与
  nullable/empty 细节；
- 由新增 Assets runner 比较 multipart metadata、Static Assets 三段上传、ID/timestamp/ETag/cursor，且只使用
  已登记规则归一化；
- 每个 runner 对自己创建的资源完成精确删除与 inventory absent 复查。

Vectorize 或 AI Search 若因账号产品权限无法执行，必须逐产品记录权限错误和未运行范围；其它产品的通过结果不能
替代它们。P7 realtime tail/Telemetry、P8 limits 与 P9 WorkerLoader 不属于本文。

## 完成条件

只有在同一冻结 revision 上先完成 P6 hosted runner 的实现与 safety review，再完成所选远端矩阵、生成清洗后的
报告、所有本次资源均复查 absent，且差异已经修复或登记为有官方依据的明确 deviation 后，才能将本文归档。
未提供凭证、preflight 失败、权限不足、runner 尚未覆盖、部分运行或清理未确认时，状态保持待验收。
