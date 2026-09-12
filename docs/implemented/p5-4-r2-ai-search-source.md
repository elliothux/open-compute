# P5.4 R2 AI Search source

状态：implemented，2026-09-11。P5.4 让 AI Search instance 直接同步同一 account 下由 open-compute
管理的 R2 bucket。R2 object 仍只保存一份；AI Search 只持久化冻结的 source identity／revision、派生
chunk、embedding、FTS 和 durable job 状态。

## 公开合同

Workers binding 与 Cloudflare v4 create/update/info surface 支持 `type`、`source`、`source_params`、
`token_id`、`sync_interval` 和 `paused`。`paused: true` 只停止 scheduled sync；initial sync、manual job、
individual item sync 和现有 index 查询仍可执行。状态保存在 canonical public config 中，重开 instance 后
仍返回同一值。例如：

```ts
await env.AI_SEARCH.instances.create({
  id: "documents",
  type: "r2",
  source: "lynx-files",
  source_params: {
    prefix: "docs/",
    include_items: ["**/*.pdf", "**/*.md"],
    exclude_items: ["**/*.tmp"],
  },
  token_id: "<GET /accounts/{account}/ai-search/tokens 返回的 id>",
  sync_interval: 3600,
});
```

- instance ID 接受 1–64 个合法字符。
- interval 接受 900、1800、3600、7200、14400、21600、43200、86400 秒；默认 21600。
- include/exclude 各最多 10 条；exclude 先执行；匹配区分大小写；`*` 不跨 `/`，`**` 可跨 `/`。
- `type: "r2"` 必须同时给出 source 和 account-scoped installation token。source 在 create 时按同
  account、ready、healthy 的逻辑 bucket 精确解析一次；内部 ResourceId 和 physical locator 不出现在响应中。
- source identity 与 jurisdiction 不可更新；filter、custom metadata observation 和 interval 更新直接作用于
  当前 Day1 模型并触发一次 reconcile，不保留旧配置路径。
- `PUT /items` 按 source key 创建或更新 R2 item；`wait_for_completion: true` 最多等待 40 秒，超时返回
  当前状态且后台 coordinator 继续处理。upload 和 `PATCH /items/{id}` 使用同一等待合同。
- R2 item 的 `source_id` 及 list filter 均使用 `r2:<bucket>`；`metadata_filter` 使用现有 Vectorize filter
  语法。排序在分页前执行：默认按 status priority 再按 `last_seen_at`，`modified_at` 按 R2 upload time
  倒序并回退到 item create time。DELETE 返回 `{ key }`。

当前公开合同依据 Cloudflare 的
[Workers binding instances](https://developers.cloudflare.com/ai-search/api/instances/workers-binding/)、
[REST create](https://developers.cloudflare.com/api/resources/ai_search/subresources/instances/methods/create/)、
[R2 data source](https://developers.cloudflare.com/ai-search/configuration/data-source/r2/)、
[syncing](https://developers.cloudflare.com/ai-search/configuration/indexing/syncing/)和
[path filtering](https://developers.cloudflare.com/ai-search/configuration/indexing/path-filtering/)。

## Authority 与生命周期

追加且未改写历史 migration 的 `ai_search_r2_sources` 表冻结 instance→bucket reference；SQLite trigger
验证 resource kind、account、bucket state/name 和 instance lifecycle，并阻止仍被引用的 bucket 删除。
`ResourceRepository::referrers()` 以 `ai_search_r2_source` 返回该引用。

per-instance schema 只有一个 v2 实现：generation locator 是 typed
`Builtin(AiSearchObjectReference) | R2(AiSearchR2ObjectReference)`。R2 revision 保存 logical
`object_version`、opaque unquoted ETag、size 和 uploaded time，不把 key 塞进 built-in object key，也不把
ETag 冒充 SHA-256。旧 v1 instance fail closed，需删除重建；没有 dual read/write、backfill 或兼容 shim。

因此 R2 generation 不会进入 built-in source-object GC 或 snapshot object enumeration。instance 删除先收敛
job 和 built-in GC，再由 locator 删除释放 bucket reference；R2 object 本身从不被 AI Search 删除。

## Reconcile 与读取

- create 持久化 initial reconcile；manual job、scheduled tick 和配置变化共用一个 durable reconciler。
- manual full sync 有 30 秒频率限制；同一 instance 同时最多一个 active reconcile，schedule missed tick
  coalesce，不补发 storm。
- 一次有上限的 control SQLite read transaction 按 raw key 获取一致 inventory：prefix 最多 100,000 个
  object，filter 后最多 10,000 个 candidate。transaction 内不做网络 I/O。
- unchanged successful revision 不 HEAD、不 parse、不调用 provider；new/changed candidate 使用 R2 配置的
  bounded HEAD concurrency 和 operation timeout。任一 authority、mutation、missing 或 revision drift 使整次
  scan retry，旧 catalog 不执行 deletion diff。
- 所有 HEAD 成功后，在一个 per-instance transaction 中创建/更新 generation 并删除完整 inventory 中消失的
  R2 item。child indexing job 完成后 parent 才成功；lease/restart 复用同一 committed diff。
- `item.sync()` 只 HEAD 指定 object：unchanged 直接返回，changed enqueue 一个新 generation，missing 保留旧
  item 等下一次完整成功 scan 确认，不扫描整个 bucket。
- parser 读取 generation 时重新验证 logical record 与 mutation fence，再用保存的 ETag conditional GET；
  version、ETag、size、uploaded time 或 stream length 漂移均 fail closed。
- item download 代理当前同名 R2 object 的授权 stream，不复制 source bytes；缺失返回 not found。

supported format 与大小准入复用唯一 document-format registry 和 parser input limit。reconcile 会先 HEAD
候选 object；有受支持扩展名时仍校验显式 MIME，extensionless key 则由受支持的 stored `Content-Type`
准入。缺失、malformed、unsupported 或 `application/octet-stream` Content-Type 都 fail closed。R2 custom metadata field
name 按 Cloudflare 当前合同不区分大小写；声明字段才会 materialize，无法转换的值静默省略，不做 truthy
coercion。item/search key 始终是 raw R2 key，`source_id` 是 `r2:<bucket>`，`checksum` 是 opaque ETag。

## 范围与限制

这是单机 SMB profile 的本地 R2 source：不接受 generic S3 endpoint、外部 Cloudflare R2 credential、crawler、
event notification、webhook 或 source detach。单次 scan 的 100,000/10,000 上限是明确本地边界，超限完整失败
并保留旧 index。Cloudflare hosted placement、replication、billing、service-token 管理和 AutoRAG 不在声明范围；
token metadata deviation 见 `OC-AI-SEARCH-TOKEN-001`。

当前正式 pin 是 `v1.20260905.0-open-compute-p1.b3e1a278`、compatibility date `2026-09-08`。公开文档已
包含 64 字符 ID 和完整 interval enum，而该 pin 附带的旧类型注释仍显示更窄边界；实现按当前公开 wire 合同
验证。`test/conformance/ai-search-r2-item-differential.ts` 已冻结 issue #52 所需的 hosted probe：完成
initial reconcile 后 pause instance，写入 extensionless R2 object，再用 `PUT /items` 和
`wait_for_completion: true` 记录创建、`source_id` 与 queryable 结果，并精确清理临时资源。当前环境没有
Cloudflare 凭据且本轮未获外部写授权，所以该 probe 尚未执行。raw-key filter、checksum、manual-overlap 及
上述 PUT 行为的 Cloudflare hosted differential 继续在
[P5 发行验收](../acceptance/p5-release-acceptance.md)保持未验证，不用本地 mock 冒充托管证据。

## 本地验证

源码与回归覆盖 strict request shape、interval/ID/filter、typed central/per-instance authority、bucket deletion
blocker、R2 reconcile、stable item identity、individual item sync、FTS deletion、zero-copy/GC exclusion、wildcard
规则和 runtime facade。real-workerd `p5-search` Gate 还会由 tenant 通过 R2 binding 写入 source object、创建
R2-backed instance、pause 后运行 explicit job、索引带有效 MIME 的 extensionless key、按 metadata/source
列出与检索、删除 item，并确认 source object 未被修改。最终验收结果以本次 issue #52 运行记录为准。
