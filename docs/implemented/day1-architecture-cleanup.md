# Day1 架构清理

记录日期：2026-08-28；完成日期：2026-08-29。

状态：**已实现、完成最终验收并归档**。清理以 `main` 的 `f6a4ba4` 为起点，
直接收敛当前 Day1 模型，不读取、迁移或兼容旧开发数据库、旧 Workflow engine、
旧私有协议、旧配置形态、旧快照身份或历史密文字节。清理前审查证据保留在下文；
完成结果与可复核命令见[验收记录](day1-architecture-cleanup-results.md)。

## 完成摘要

- 清单 1–17 与 artifact GC 引用时序均已完成；每个领域只保留一个当前生产实现。
- Workflow execution/caller capability 当前统一为 `1`；V1/V2 双引擎、旧 selector、
  wrapper 分流与旧 Gate 入口已删除，durable waiting/retry/event/replay 成为唯一模型。
- Control/Scheduler SQL 直接定义当前 schema；平台历史 upgrade CLI、范围矩阵、
  schema backfill 与旧快照/格式可读列表已删除，当前身份严格相等校验保留。
- KV、Scheduler 配置、Secret crypto/AAD、runtime token、binding/supervisor/descriptor 装配、
  capability 方法、配额/restore、失败重放、指标、load/soak 与 artifact GC 均收敛到当前 owner。
- 最终源码指纹 `eb426598f28eeb353333d91b2dd81fbef6f8eeae657733407396f029cff68e71`；
  coverage 32 目标单轮通过，Rust 行覆盖率 **90.02%**（54,260 / 60,273）。
- 最终 Gate 报告 `.temp/gate-run/20260829T121329-f77e4f98/report.json`：round 1
  32 目标 / 662 用例，round 2、3 各 17 目标 / 42 个登记时序用例，全部通过。
- 使用已存在且校验通过的 `v1.20260826.1` Darwin ARM64 archive/binary；没有下载 runtime、
  重置本地数据、删除失败证据、打包、发布或执行特权 egress。

## 清理边界

以根目录 [AGENTS.md](../../AGENTS.md) 的 Day1 约定为准：open-compute 尚未用于生产，
不为以前的开发版本、数据、配置、平台 schema、快照、内部协议或字节格式保留兼容路径。

唯一的兼容例外，是本项目已声明支持的 Cloudflare 官方 API 确实要求的对外兼容行为。
每个例外必须记录官方来源、具体行为、适用的日期/flag 或版本范围、相关 workerd pin 约束，
以及验证该行为的回归测试。官方存在某个功能，不等于要求保留我们实现该功能时产生的旧架构。
本次清理也不自动扩大 Cloudflare API 支持面。

以下能力不能因清理而丢失：

| 能力 | 应保留的内容 | 不构成保留理由的内容 |
| --- | --- | --- |
| workerd 兼容设置 | 当前 pin 支持范围内的日期和 flags 所规定的行为。[官方说明](https://developers.cloudflare.com/workers/configuration/compatibility-dates/) | 旧 open-compute 配置、runtime 布局或私有适配器 |
| Workflow | 当前已支持的 step、retry、sleep、event、实例生命周期与 replay 语义。[官方 API](https://developers.cloudflare.com/workflows/build/workers-api/) | 本项目自定义的 capability V1/V2 双引擎及旧调用方适配 |
| KV | 当前已支持的文本、JSON、二进制、stream 和 metadata API；单键 `get()` 默认文本。[官方 API](https://developers.cloudflare.com/kv/api/read-key-value-pairs/) | 旧文本 executor、私有 JSON/frame 双通道、Gate 专用扩展 |
| D1 | 用户数据库的 SQL migrations 产品能力。[官方说明](https://developers.cloudflare.com/d1/reference/migrations/) | 平台 control/scheduler 数据库的历史升级链 |
| Worker 版本、Secrets、R2 | 当前已支持的部署版本、secret binding 与 S3/R2 协议语义。[版本](https://developers.cloudflare.com/workers/versions-and-deployments/)、[Secrets](https://developers.cloudflare.com/workers/configuration/secrets/)、[R2](https://developers.cloudflare.com/r2/api/s3/api/) | 旧平台发布兼容矩阵、旧密文格式、历史快照指纹 |

事务、外键、锁、加密、摘要校验、不可变部署、同一实现下的重启/崩溃恢复也是当前正确性要求，
即使由我们自行实现也必须保留。普通配置默认值、当前格式版本标记和拒绝未知格式的校验，
不因名字包含 default/version 就属于历史兼容。

## 实施清单（已完成）

### 1. 统一 Workflow 执行模型

证据：[Loader 分流](../../packages/runtime/src/loader/bindings.ts)、[模块装配](../../packages/runtime/src/loader/modules.ts)、
[调度器](../../crates/service/src/scheduler/workflow.rs)、[后端路由](../../crates/service/src/workflow_backend.rs)。
`packages/runtime/src/workflows/` 仍有 facade、runner、JSON codec 的两套实现；
`/internal/workflows/v1/runs/` 与 `/internal/workflows/v2/runs/` 同时存在。

- [x] 将当前已支持的 durable Workflow 能力作为唯一实现，移除旧 V1 引擎、模块、transport、
  scheduler/storage 分支及 capability 1/2 选择逻辑；同步工具链声明、类型和生成资产。
- [x] 删除 [deployment 输入](../../crates/workers/src/pipeline.rs) 中的 `legacy_binding_capability`
  及 [Workflow HTTP 输入](../../crates/service/src/workflow_http.rs) 中的 `legacy_capability`，
  不再通过省略字段选择旧执行语义。
- [x] 保留当前模型中的不可变代码/部署身份、实例冻结目标、replay、权限校验及失败恢复。
  区分用户部署版本与平台引擎版本，不把业务版本身份一起删除。

### 2. 将平台 SQL 收敛为当前 schema

证据：[Control 初始化/迁移](../../crates/storage/src/migrations.rs)、
[Scheduler 初始化/迁移](../../crates/storage/src/scheduler.rs)。当前注册了 14 个 Control SQL 文件和
8 个 Scheduler SQL 文件，包含历史表重建、旧数据复制和版本专用校验。

- [x] 按领域直接定义当前 schema，移除仅为旧开发数据库保留的增量升级分支。
  SQL 可以继续分文件，不应为了消除迁移而把所有 DDL 平铺到一个大文件。
- [x] 删除 Control Workflow 重建（原 `crates/storage/migrations/013_workflow_durable_waiting.sql`）
  与 Scheduler Workflow 重建（原 `crates/storage/scheduler-migrations/006_workflow_durable_waiting.sql`）
  的旧表快照、复制和历史字节对比；同步移除 `verify_legacy_histories` 等专用迁移逻辑。
- [x] 删除 operation sequence 回填（原 `crates/storage/migrations/014_workflow_operation_sequence.sql`）
  与 operation progress 回填（原 `crates/storage/scheduler-migrations/007_workflow_operation_progress.sql`），
  将当前字段、约束和 fencing 规则直接纳入新建 schema，保留它们的当前正确性用途。
- [x] 同步 SQL checksum/build wiring、当前 schema 校验、fixtures 和故障测试。
  验证空库初始化、当前库重开、事务中断和损坏拒绝；不自动修复、重置或删除旧本地数据库。

### 3. 移除平台历史版本升级产品路径

清理前证据：`crates/service/src/upgrade_cli.rs`、`crates/storage/src/upgrade.rs` 和
[发布元数据](../../crates/core/src/release_identity.rs) 曾保留 `upgrade check/apply`、
`upgrade_from_control_schema_min`、`upgrade_from_platform_versions` 和
`restore_compatible_platform_versions`。

- [x] 移除跨旧平台版本的升级入口、范围判定、迁移调用、专用回执与兼容矩阵，
  同步调用方、operator 文档、发布工具和历史升级测试。当前保留的共享检查见
  [schema inspection](../../crates/storage/src/schema_inspection.rs)，恢复拒绝矩阵见
  [当前 snapshot Gate](../../crates/service/tests/p1_snapshot_restore/staged_validation.rs)。
- [x] 保留当前 release/pin/资产身份、当前 schema 检查、备份完整性、当前实现的快照恢复，
  以及 Worker 部署指针的 promote/rollback；不要直接删除共享的检查代码。
- [x] 同时收敛只为多版本读取保留的 KV/D1 schema min/max、`OfflineSchemaState` 范围及
  `readable_object_formats` 版本列表。当前生产方只写一个版本；保留单一当前格式身份与严格
  相等校验，不把“删升级 CLI”当成所有生产/快照消费者都已收敛。

### 4. 统一 KV 私有协议与执行接口

证据：[Binding backend](../../crates/service/src/binding_backend.rs) 根据 Content-Type 在 `dispatch`
与 `dispatch_frame` 之间分流；`KvBindingExecutor` 同时保留文本 `get/put/delete` 与 `KvCommand`。
[SQLite backend](../../crates/service/src/kv_backend.rs) 还有反向适配，
[runtime transport](../../packages/runtime/src/kv/transport.ts) 暴露 `echoStream`。

- [x] 保留一套当前内部协议和执行接口，删除旧 JSON 请求分流、文本 executor 兼容默认实现、
  双向适配及不再需要的协议常量。
- [x] 将旧 Gate 的 `echoStream` 等测试需求放回测试夹具，不作为生产 KV API 延续。
  更新 [旧 KV 接口测试](../../crates/service/src/kv_backend_tests.rs) 和相关真实 runtime Gate。
- [x] 保留用户可见的文本默认读取、二进制/stream/metadata 功能，以及流式背压、资源 pin、
  权限、容量和超时结果不确定性的覆盖。

### 5. 移除 Scheduler 旧配置回退

证据：[SchedulerConfig](../../crates/core/src/config.rs) 的顶层 `claim_batch`、
`pools: Option<SchedulerPoolsConfig>` 及 `pool()` 在缺少 pools 时构造旧 Alarm 设置。

- [x] 使用唯一的 pools 配置结构及正常默认值，删除旧 Alarm 配置形态与回退。
  保留全局 `max_in_flight` 与各 pool 的并发/批次/公平调度约束。
- [x] 同步默认配置、配置解析、CLI 展示和
  [旧配置兼容断言](../../crates/core/src/config_tests.rs)，验证当前配置默认值和非法配置拒绝。

### 6. 移除历史序列化和哈希保护

证据：[WorkflowsConfig](../../crates/core/src/workflow.rs) 仍通过 `default_parallel_steps` 等谓词
省略新增字段；[Workflow descriptor](../../crates/storage/src/workflows/helpers.rs) 的 `version_digest`
仍只给 capability 2 加 `schemaVersion`，以保留 V1 编码。

复核更新：当前工作树中的 [SnapshotPolicyV1](../../crates/service/src/capabilities.rs) 已改为始终
包含 workflows 字段；[对应测试](../../crates/service/src/capabilities_tests.rs) 已改为检查当前配置。
这是已有未提交修改的源码观察，本次未执行其行为测试，不将其记作完成验收。

- [x] 统一当前配置和 descriptor 的规范编码，删除仅为历史字节相等保留的省略/分支及
  [旧序列化断言](../../crates/core/src/workflow_tests.rs)。
- [x] 连同已有快照修改一起验证：当前配置与权限等有效输入变化能反映到摘要，规范编码可复现，
  篡改与错误身份被拒绝；不再要求旧开发版本的哈希保持不变。

### 7. 统一 Secrets 加密接口与 AAD

证据：[SecretCrypto](../../crates/storage/src/crypto.rs) 保留 `encrypt/decrypt` 和
`encrypt_revision/decrypt_revision`，`associated_data` 根据 revision 是否存在选择 AAD 1/2。
当前 [部署写入](../../crates/workers/src/pipeline.rs) 和
[RuntimeSource 读取](../../crates/workers/src/runtime_source.rs) 使用 revision 绑定路径；
旧接口的仓库业务调用仅见于测试。

- [x] 保留唯一的当前加解密入口与 revision 绑定 AAD，移除旧无 revision API、旧格式选择和重复实现。
- [x] 将旧接口安全测试迁到当前入口，保留 nonce、key identity、account/worker/deployment/secret/revision
  绑定、密文篡改及错误上下文拒绝等覆盖；不读取或转换旧密文来维持开发数据兼容。

### 8. Runtime 包与测试流程收敛

已于 2026-08-29 完成本机实现与验收，见 [Runtime 包与测试流程整理](runtime-and-test-layout.md)
和[实测记录](runtime-and-test-layout-results.md)：workspace 690 个用例、90.16% 行覆盖率，
最终 23 目标各三轮全部通过。跨平台与正式发行资格另见[活动验收计划](../runtime-layout-release-acceptance.md)。
以上是当次运行记录；后续[按用例分轮改造](test-repetition.md)已完成本机验收，采用完整一轮、时序补两轮，
现行验收规则以[测试节奏](../references/testing.md)为准，不改写上述历史结果。

- [x] 将 `runtime/` 移至 `packages/runtime/`，生成目录改为 `dist/` 并取消 Git 跟踪；
  同步源码、配置、正式 pin、Rust 内嵌/manifest 消费者、workspace、CI 和发行构建路径。
- [x] 从不含 `dist/` 的干净源码快照构建并验证可复现资产，生产启动继续离线且不依赖 JS 工具链。
- [x] 删除已完成的 POC 上游能力探测、旧原型断言、重复测试及其孤立依赖；将仍需维护的产品
  回归与所需 harness、fixtures 统一移入 `test/`，保留当前安全、完整性和恢复覆盖。
- [x] 移除独立 POC pin、旧 G0 入口及相关必跑依赖；保留历史报告、已知限制和既有失败证据。
- [x] Gate 默认一轮，最终显式三轮；消除内部固定三轮、递归调用和重复 target，每轮各目标一次。
- [x] 减少重复构建、类型检查、初始化和无效等待；完整检查与 coverage 不随 Gate 轮次重复，
  记录实际执行次数与耗时，coverage 门槛仍为 90.00%。

## 三轮全库复核新增项（2026-08-29）

用户追加全库复核后，按三个不同角度检查当前未提交工作树：第一轮检索兼容、默认值、
未消费输入和版本分支；第二轮追踪生产入口、authority、runtime 与恢复调用链；第三轮对照
能力声明、现有测试、维护文档及归档记录。覆盖六个 Rust crate、runtime/toolchain、SQL、
构建/运维脚本、测试调度与 CI；没有把生成目录或关键词命中数当成有效问题数量。

下列 9–13 是原清单未具体记录、随后纳入本阶段并完成的清理范围。实现仍只覆盖已证明的
Day1 当前要求，没有扩大产品能力。清理前检索记录继续作为发现证据保留。检索记录在 `.temp/day1/` 的
`compatibility-inventory.txt`、`incomplete-inventory.txt`、`defaults-inventory.txt`、
`ignored-inputs-inventory.txt`、`version-branches-inventory.txt` 与 `failclosed-inventory.txt`。

### 9. 删除无限配额旧入口，补齐 KV/D1 restore 的数量限制

优先级：高。不是仅供旧测试使用的死 API，而是当前生产请求可达的半截配额改造。

证据：[ResourceRepository](../../crates/storage/src/resources.rs) 的 `reserve_create()`
仍转调 `reserve_create_with_limit(input, u32::MAX)`。普通
[ResourceController 创建](../../crates/workers/src/resource_lifecycle.rs) 已传入
`hardening.max_resources_per_kind_per_account`，但
[KV restore](../../crates/service/src/kv_http.rs) 的 `restore_downloaded_namespace()` 与
[D1 restore](../../crates/service/src/d1_http_backup.rs) 的 `restore_downloaded_database()`
仍调用无限额入口。文件字节配额与磁盘 admission 不是资源数量配额，不能替代它。

- [x] 当前资源预留入口必须显式接收并在同一事务中执行数量限制；删除无限额默认包装。
  两类 restore 都使用当前 hardening 配置，不能只给普通 create 补限额。
- [x] 同时检查 Worker、route 和 deployment 的 `u32::MAX` 包装，迁移仅有的测试调用方；
  保留一个有明确配额的当前入口，避免下一条生产路径再次接错。
- [x] 回归覆盖：达到上限时普通 create 与新 restore 均拒绝；已有恢复的幂等重放/崩溃续做
  不被误判成新增资源；并发 restore 不能超过限额；拒绝后没有新权威行或残留 staging。

该缺口随后以显式当前配额入口修复；普通 create、KV/D1 restore、并发/幂等与清理路径
已纳入当前测试，并随最终 workspace、coverage 与三轮 Gate 通过。

### 10. 删除 runtime 同 token 的旧测试分支

优先级：中。证据：[digest](../../crates/runtime/src/digest.rs) 的
`render_config_with_tokens()` 明确拒绝相同 token，但生产编译路径调用的
`digest_for_with_tokens_and_policy()` 在 token 相同时反而走单 token renderer，再手动替换
binding token，绕过上述检查。[旧测试](../../crates/runtime/src/tests.rs) 的 `compile_req()`
仍把同一个 token 传给两个字段；`digest_for()` 也保留这种测试形态。

正常 supervisor 已生成两个独立随机 token；本发现不是“当前 HTTP 请求能选 token”，
而是可调用编译接口的安全约束被旧 fixture 形态分裂。

- [x] 统一 renderer、digest 和编译入口的 token 校验，始终要求两个不同的有效 token。
  删除单 token adapter 和未使用的 `lock` 参数，避免仅修一个未被生产直接调用的 helper。
- [x] 测试夹具生成两个 token；通过真实 `compile_static_config()` 验证相同 token 拒绝，
  保留任一 token 变化影响 digest、generation 轮换/撤销与 secret hygiene 覆盖。

### 11. 收敛按阶段追加的装配包装与仅供测试的生产 API

优先级：中。证据：[binding backend](../../crates/service/src/binding_backend.rs) 仍保留
`serve_binding_backend` → `with_metrics` → `with_r2` → `with_products` →
`with_products_and_do_config` → `with_scheduler` 六层入口，前五层只转发并补空产品/默认配置；
当前 [run](../../crates/service/src/run.rs) 直接调用最后一层。
[supervisor](../../crates/runtime/src/supervisor/mod.rs) 的 `new` / `new_with_external_services` /
`new_with_external_services_and_auth` 也逐层转发到完整装配入口；
[descriptor](../../crates/workers/src/descriptor.rs) 的旧 `new()` 为产品 binding 补空列表，
而当前 deployment 与 RuntimeSource 都调用完整入口。

另有 `UnavailableKvBindingExecutor`、`FnCompiler`、`future_resource_paths()` 和
`expected_directories()`：仓库调用方只有测试，但类型/函数仍在生产库中无条件公开。
[data-dir](../../crates/storage/src/data_dir.rs) 还始终创建并要求 `runtime/previous/`，
全库没有读写其内容的运行流程，只有布局与校验本身。

- [x] 每个 owner 保留直接的当前装配入口；测试也传显式的当前参数，不保留按旧阶段堆叠的
  无逻辑包装。不要因此把确有语义的 optional 服务或 provider 能力检查一概删除。
- [x] 仅测试用途的 executor/compiler/布局断言移到测试模块或显式 `test-support` 边界。
  普通构建不导出测试替代物。
- [x] 当前初始化与布局校验不再要求没有消费者的 `runtime/previous/`，同步当前布局测试；
  不删除或迁移已有数据目录里的内容，保留 runtime 本身的校验、恢复与证据边界。

### 12. 修正 capability methods 与实际 facade 不一致

优先级：中。[能力注册](../../crates/service/src/capabilities.rs) 声明 `KV.getBulk` 与
`R2.deleteMany`，但当前 [KV facade](../../packages/runtime/src/kv/transport.ts) 提供的是
`get(string[])` / `getWithMetadata(string[])`，
[R2 facade](../../packages/runtime/src/r2/facade.ts) 提供的是 `delete(string[])`；两个声明的方法
实际不存在。`ProductCapabilityV1.methods` 的契约明确是 method names，不能用内部操作名代替。
[conformance 测试](../../crates/service/tests/p1_conformance.rs) 只是重复断言同一份错误列表，
没有验证这两个名字可调用。

- [x] capability 只列实际支持的方法，批量行为通过重载/支持面说明表达；不要为了清单造出
  新的非标准 alias。同步 metadata 消费者和 conformance 断言。
- [x] 让能力声明与真实 facade 的对应行为有交叉验证，保留批量读取/删除的成功和失败用例。

本轮复用 runtime 的 Rolldown helper 检查当前源码类原型，确认两个方法都是 `undefined`；
记录为 `.temp/day1/facade-method-review.json`。这是 API 形状检查，不是 stock-workerd Gate。

### 13. 补齐 load/soak 脚本的当前输入与证据契约

优先级：中。[load](../../test/load-p1.sh) 与 [soak](../../test/soak-p1.sh) 接受 `--seed`，
但 seed 只进入最终 JSON，没有传给负载或故障执行；当前调度实际固定。
它们还硬编码 runtime cache 的 release 路径，并写固定的 `target/p1-results/...` 目录，
下次启动会截断旧日志。审查时 soak 还调用 `p1_upgrade`，这一调用已随第 3 项移除；
其余脚本缺口已在本阶段修复；长时 1h/24h 运行仍由 [P1 剩余验收](../p1-release-acceptance.md)
单独追踪，不属于本阶段本机最终 Gate。

- [x] 若保留固定负载，就移除无效 seed 参数并如实记录固定 schedule；如果约定需要不同 seed
  的负载，必须实际消费并能复现，不能只改报告。长时负载设计不由本次审查自动扩大。
- [x] 使用显式、验证过的 runtime 输入和当前唯一测试调度；清除对历史 upgrade 用例的依赖。
  长时 soak 不是日常开发 Gate，不为“统一”再引入每轮完整三次重跑。
- [x] 新运行使用 `.temp/<purpose>/<unique-run>/`，隔离并保留失败证据、拒绝覆盖；
  不移动、删除或改写已有 `target/p1-results/` 历史报告。
- [x] 脚本参数、计划与证据路径可以用有界测试验证；1h/24h 的实际持续运行仍由活动验收计划
  跟踪，不能把脚本修正记作长时验收通过。

### 纳入范围：artifact GC 的引用时序缺口

优先级：高；静态发现，未做受控交错复现，不计作已完成的行为验证。
[Worker maintenance](../../crates/service/src/run.rs) 第二次读 `referenced_artifacts()` 失败时，
回退到本轮更早的引用集合并继续 GC；[ArtifactStore](../../crates/artifacts/src/store.rs) 删除时
只看传入集合与对象 `LastModified`。[deployment pipeline](../../crates/workers/src/pipeline.rs)
先上传/验证 artifact 再提交引用；复用已有相同内容对象不会刷新它的 `LastModified`。

因此，一个已超过 grace 的无引用对象若正被重新部署，旧集合不包含新引用；即使移除失败回退，
引用快照和实际删除之间也仍需证明与新部署的互斥。当前源码未见覆盖这个窗口的共享 artifact
预留/fence。这个问题是当前数据完整性流程未闭合，不是旧格式兼容。

- [x] 先加受控交错复现：旧对象重用、引用重读失败、引用读取后新增部署三种窗口；确认现有
  admission/pin 的实际保护范围，不能把“有 grace”当成并发证明。
- [x] 修复需协调 artifact 创建/复用、持久引用与 GC owner；先确认扩展实施范围，再设计唯一
  当前路径。不加历史数据修复，不以关闭完整性断言换取测试通过。

### 本轮排除的误报

- Workflow 双引擎、SQL 回填、升级 CLI、旧 AAD 等已在 1–7，不能重复计成新发现。
- D1 bookmark/session、R2 multipart、DO output gate 与 hibernation 的明确限制已有支持面/偏差
  记录；Static Assets 与 Service Bindings 有活动计划，不是偷偷宣称已完成的占位实现。
- KV `cacheTtl`、R2 provider 的 single-delete 路径、DO alarm shim 与未知 schema 拒绝不能只凭
  名字认作历史兼容；它们分别涉及当前对外协议、provider 支持面、原生边界与完整性校验。
- Worker limits 只接受 `{"profile":"default"}`，runtime 应用固定 profile；未发现接受任意
  自定义 CPU/subrequest 数值却静默忽略的路径，不按这个猜测报问题。
- release metadata 的静态 conformance 标识不是本次或未来构建通过 Gate 的证据；实际运行
  仍必须看绑定源码与输入的报告，不能由 capability 输出推断验收通过。

## 追加三轮复核：配置、失败重放与可观测性（2026-08-29）

在已有 1–13 和 artifact GC 记录之外，再按「声明是否有消费者」「生产调用链是否贯通」
「文档与测试是否掩盖缺口」三个角度复核。以下 14–17 均是新增审查记录，不是实现或验收完成，
也不自动扩展已经授权的实施范围。未修改生产源码或现有测试。
对应源码摘要和静态交叉断言保存在
`.temp/day1/review-772da6c1f1/source-cross-check.json`；这些断言检查当前源码，不是 HTTP 复现或
stock-workerd Gate。原先仅列出这些字段/指标的阶段设计，不构成缺口已经被记录或实现的证据。

### 14. 移除或补齐没有消费者的配置

优先级：中。两处配置都能通过解析与静态校验，但没有相应的生产行为。

- [ServerConfig](../../crates/core/src/config.rs) 接受 `server.trusted_proxies` 并校验 CIDR；
  除配置本身外，唯一消费者是 [support bundle](../../crates/service/src/support_bundle.rs) 中的数量展示。
  [runtime bridge](../../crates/service/src/runtime_bridge.rs) 无条件删除 `Forwarded` / `X-Forwarded-*`，
  并以 `http://` 构造原始 URL，没有按连接来源与代理名单恢复 scheme/host 的分支。
  当前是配置无效，不是已证明存在信任伪造 header 的漏洞；清理不能直接放行客户端 header。
- `diagnostics.max_failed_starts` / `diagnostics.max_bytes` 仅出现在配置定义、默认值、校验及测试。
  [data-dir](../../crates/storage/src/data_dir.rs) 创建 `diagnostics/failed-starts/`，却没有报告写入与保留流程。
  [supervisor](../../crates/runtime/src/supervisor/mod.rs) 只保存最后一份内存 `ProcessDiagnostics`，
  [日志收集](../../crates/runtime/src/supervisor/logs.rs) 使用固定的 16 KiB tail 上限；这不是配置声明的
  失败报告数量/目录字节保留策略。未发现维护文档明确承认这两个配置尚未生效。

- [x] 明确当前支持面：不支持的配置应删除或明确拒绝，不能继续接受并无声忽略；若要补齐能力，
  先确认实施范围与 owner，不因已有字段就默认扩大功能。
- [x] 若保留代理支持，回归直接连接、不受信代理、受信代理与伪造 forwarded header；
  若保留诊断保留策略，验证实际报告写入、数量/字节边界、secret redaction 与失败处理。
- [x] 同步默认配置、CLI/config 检查、文档和测试。不删除已有 diagnostics 或保留证据。

### 15. 补齐当前失败码的幂等重放

优先级：中。当前 quota/admission 改造已接入首次请求，但失败重放仍使用较早阶段的局部错误码表。

证据：[Worker 创建](../../crates/service/src/workers_http.rs) 在 `run_idempotent()` 内调用
`create_worker_with_limit()`；[存储 authority](../../crates/storage/src/workers.rs) 达到数量上限返回
`QuotaExceeded`，随后失败码持久化。同键再次请求进入 `replayed_failure()`，其 `error_code()`
白名单没有 `QuotaExceeded`、`AdmissionBusy`、`StoragePressure` 或 `PlatformUnavailable`，
统一变成 `Internal`。按同一文件的 HTTP 映射，配额失败首次是 429，重放变成 500。

[Deployment pipeline](../../crates/workers/src/pipeline.rs) 也会保存 admission 或 deployment 配额失败；
[parse_failure_code](../../crates/workers/src/pipeline/validation.rs) 同样缺少上述当前错误码。
这不涉及旧数据兼容：同一二进制刚写入的合法失败记录，自己的读取路径就无法完整识别。
[现有重放单测](../../crates/service/src/workers_http_tests.rs) 验证的是白名单内的 `WorkerNotFound`，
没有覆盖这组后来接入的失败码。

- [x] 让当前持久化失败码的写入与读取共享明确的类型/契约，移除分散且不完整的旧白名单；
  未知或损坏的持久化记录仍必须拒绝，不添加历史别名或宽松补值。
- [x] 回归 Worker/route/deployment 的首次失败、同键重放及进程重启后重放，确认当前错误码与
  HTTP 分类一致、没有重复执行 mutation；成功和冲突重放覆盖继续保留。
- [x] 复查 resource/Queue/backup 各自的失败持久化消费者，但不把尚未证明可达的缺项一概报成 bug。

### 16. 不再把尚未采集的 DO 指标输出为零

优先级：中。证据：[DO metrics](../../crates/service/src/metrics_do.rs) 对外导出
`oc_do_websocket_active` 与 `oc_do_storage_bytes`，后者 HELP 明确写成 runtime 报告的 localDisk 字节数。
但 `set_do_runtime_gauges()` 的三个生产调用都固定传入前两个参数 `0, 0`：
[启动和周期维护](../../crates/service/src/run.rs) 两处，
[DO dispatch admission](../../crates/service/src/binding_backend.rs) 一处。没有实际 WebSocket 计数或
localDisk 字节采集者；更新 watermark 还会再次写入这两个零值。

[metrics 单测](../../crates/service/src/tests.rs) 直接调用 setter 注入 `2` 和 `4096`，只证明输出格式，
不能证明生产采集成立。基础 WebSocket 已有实现，这个缺口不能归入已记录的 hibernation No-Go。

- [x] 只有实际观测值才能作为 gauge 输出；为当前可采集能力接通 owner，无法可靠获得的指标
  应明确标为未提供，不能用零代表未知。保留实际工作的磁盘 watermark。
- [x] 测试应经过生产采集路径验证非零、清理/关闭及 runtime 重启后的变化；保留低基数与
  secret hygiene，不用 fixture 直接设置指标代替行为验证。
- [x] 同步 capability/运维说明和固定 metrics 预算；不能凭指标名称宣称采集已经实现。

### 17. 收敛 Scheduler 的旧指标别名与半截多 workload 导出

优先级：中。证据：[metrics scheduler](../../crates/service/src/metrics_scheduler.rs) 同时输出
旧 `oc_scheduler_*` 与后加的 `open_compute_scheduler_*`。至少三组直接复用同一个计数器：
`in_flight`、`claim_total`，以及旧 `claim_expired_total` 对应新 `lease_recovery_total`。
这是同一 Alarm 事实的两套发布名称，不是官方 Cloudflare API 所要求的兼容行为；
原清单第 5 项只涉及 Scheduler 配置，没有覆盖指标出口。

同一文件为 `ready`、`stale_completion`、`pool_state` 保存四类 workload 的数组，
[runner](../../crates/service/src/scheduler/runner.rs) 与各 workload owner 也确实写入 Queue/Cron/Workflow 数据，
但通用导出始终索引 `SchedulerKind::Alarm`，并硬编码 `kind="do_alarm"`。
其余三类数据已采集却没有从这些通用指标输出，不能把产品专属指标当成这条未完成路径的实现证据。

- [x] 保留一套当前 Scheduler 指标名称，删除同义旧出口，更新实际消费者、预算和断言；
  不误删语义不同的 scheduled/ready/backlog、Alarm 专属结果或恢复指标。
- [x] 让现有 workload 状态的采集与导出一致，覆盖 Queue/Cron/Workflow 的非零值、暂停/backoff、
  stale completion 和恢复；不保留只写不读的计数器。
- [x] 核对最终渲染的 family/label 唯一性和固定序列预算。旧测试同时断言两套名称不是兼容义务。

本次另行排除：`lease_guard_ms` 只参与配置校验是有效用途，它约束
`claim_lease_ms >= dispatch_timeout_ms + lease_guard_ms`，不是无消费者配置；RAII 的 `_pin` / `_lease`
负责保有资源，不因未读取字段就属于占位。已有 D1 session、DO placement/hibernation 等明确限制
继续按现行偏差记录处理，不重复算作新增问题。

## 执行与验收

先明确唯一 Workflow 模型，再配套收敛 schema、升级入口、descriptor 和测试；KV、Scheduler 配置、
Secrets 可按各自领域独立清理。每项都应同步修改生产调用方、fixtures、生成资产及当前文档。
旧设计文档中的兼容要求不凌驾于 Day1 约定；它们的历史结果也不应被重写为新实现的验证结果。

- [x] 将保留旧行为的断言改为当前模型的成功、失败、安全、完整性和恢复测试。
  测试曾经通过不是保留旧实现的理由，也不能通过删除必要断言降低验收强度。
- [x] 开发期间按 [测试节奏](../references/testing.md) 运行相关单轮检查；完成实现与审查、源码冻结后，
  先执行 [AGENTS.md](../../AGENTS.md) 要求的静态检查与一次 coverage，再统一执行完整 workspace
  一轮及登记时序用例的两轮追加，保留真实 workerd 路径和逐项执行证据。
- [x] 修改 TypeScript 时通过严格类型检查、行为测试、构建和生成资产一致性检查；
  不手改 `packages/runtime/dist/` 生成产物。
- [x] 完成记录写明实际修改范围、源码基线、执行的检查、结果与剩余项。
  未运行、缺少环境、只有源码修改或未满足对应逐用例轮数，都不能标为最终验收完成。

本次文档/策略变更仅需 `git diff --check` 与路径、来源和状态核对，不重跑 Rust 或 runtime Gate。
本清单不授权清空现有数据库、删除失败证据、下载 runtime、发布或执行需要提权的命令。
