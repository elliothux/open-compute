# Day1 架构清理清单

记录日期：2026-08-28。

状态：已确认需要清理，尚未完成整体验收。本文件记录代码审查结果、清理边界和验收要求，
不代表清理已经实施。各项以其单独记录的实施与验收状态为准。
记录依据包括当前未提交工作树；实施前应重新核对源码，避免覆盖其他改动。

## 清理边界

以根目录 [AGENTS.md](../AGENTS.md) 的 Day1 约定为准：open-compute 尚未用于生产，
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

## 待办

### 1. 统一 Workflow 执行模型

证据：[Loader 分流](../packages/runtime/src/loader/bindings.ts)、[模块装配](../packages/runtime/src/loader/modules.ts)、
[调度器](../crates/service/src/scheduler/workflow.rs)、[后端路由](../crates/service/src/workflow_backend.rs)。
`packages/runtime/src/workflows/` 仍有 facade、runner、JSON codec 的两套实现；
`/internal/workflows/v1/runs/` 与 `/internal/workflows/v2/runs/` 同时存在。

- [ ] 将当前已支持的 durable Workflow 能力作为唯一实现，移除旧 V1 引擎、模块、transport、
  scheduler/storage 分支及 capability 1/2 选择逻辑；同步工具链声明、类型和生成资产。
- [ ] 删除 [deployment 输入](../crates/workers/src/pipeline.rs) 中的 `legacy_binding_capability`
  及 [Workflow HTTP 输入](../crates/service/src/workflow_http.rs) 中的 `legacy_capability`，
  不再通过省略字段选择旧执行语义。
- [ ] 保留当前模型中的不可变代码/部署身份、实例冻结目标、replay、权限校验及失败恢复。
  区分用户部署版本与平台引擎版本，不把业务版本身份一起删除。

### 2. 将平台 SQL 收敛为当前 schema

证据：[Control 初始化/迁移](../crates/storage/src/migrations.rs)、
[Scheduler 初始化/迁移](../crates/storage/src/scheduler.rs)。当前注册了 14 个 Control SQL 文件和
8 个 Scheduler SQL 文件，包含历史表重建、旧数据复制和版本专用校验。

- [ ] 按领域直接定义当前 schema，移除仅为旧开发数据库保留的增量升级分支。
  SQL 可以继续分文件，不应为了消除迁移而把所有 DDL 平铺到一个大文件。
- [ ] 删除 [Control Workflow 重建](../crates/storage/migrations/013_workflow_durable_waiting.sql)
  与 [Scheduler Workflow 重建](../crates/storage/scheduler-migrations/006_workflow_durable_waiting.sql)
  的旧表快照、复制和历史字节对比；同步移除 `verify_legacy_histories` 等专用迁移逻辑。
- [ ] 删除 [operation sequence 回填](../crates/storage/migrations/014_workflow_operation_sequence.sql)
  与 [operation progress 回填](../crates/storage/scheduler-migrations/007_workflow_operation_progress.sql)，
  将当前字段、约束和 fencing 规则直接纳入新建 schema，保留它们的当前正确性用途。
- [ ] 同步 SQL checksum/build wiring、当前 schema 校验、fixtures 和故障测试。
  验证空库初始化、当前库重开、事务中断和损坏拒绝；不自动修复、重置或删除旧本地数据库。

### 3. 移除平台历史版本升级产品路径

证据：[升级 CLI](../crates/service/src/upgrade_cli.rs)、
[离线 schema 升级](../crates/storage/src/upgrade.rs)、
[发布元数据](../crates/core/src/release_identity.rs)。当前仍有 `upgrade check/apply`、
`upgrade_from_control_schema_min`、`upgrade_from_platform_versions` 和
`restore_compatible_platform_versions`。

- [ ] 移除跨旧平台版本的升级入口、范围判定、迁移调用、专用回执与兼容矩阵，
  同步调用方、operator 文档、发布工具和 [历史升级测试](../crates/service/tests/p1_upgrade.rs)。
- [ ] 保留当前 release/pin/资产身份、当前 schema 检查、备份完整性、当前实现的快照恢复，
  以及 Worker 部署指针的 promote/rollback；不要直接删除共享的检查代码。

### 4. 统一 KV 私有协议与执行接口

证据：[Binding backend](../crates/service/src/binding_backend.rs) 根据 Content-Type 在 `dispatch`
与 `dispatch_frame` 之间分流；`KvBindingExecutor` 同时保留文本 `get/put/delete` 与 `KvCommand`。
[SQLite backend](../crates/service/src/kv_backend.rs) 还有反向适配，
[runtime transport](../packages/runtime/src/kv/transport.ts) 暴露 `echoStream`。

- [ ] 保留一套当前内部协议和执行接口，删除旧 JSON 请求分流、文本 executor 兼容默认实现、
  双向适配及不再需要的协议常量。
- [ ] 将旧 Gate 的 `echoStream` 等测试需求放回测试夹具，不作为生产 KV API 延续。
  更新 [旧 KV 接口测试](../crates/service/src/kv_backend_tests.rs) 和相关真实 runtime Gate。
- [ ] 保留用户可见的文本默认读取、二进制/stream/metadata 功能，以及流式背压、资源 pin、
  权限、容量和超时结果不确定性的覆盖。

### 5. 移除 Scheduler 旧配置回退

证据：[SchedulerConfig](../crates/core/src/config.rs) 的顶层 `claim_batch`、
`pools: Option<SchedulerPoolsConfig>` 及 `pool()` 在缺少 pools 时构造旧 Alarm 设置。

- [ ] 使用唯一的 pools 配置结构及正常默认值，删除旧 Alarm 配置形态与回退。
  保留全局 `max_in_flight` 与各 pool 的并发/批次/公平调度约束。
- [ ] 同步默认配置、配置解析、CLI 展示和
  [旧配置兼容断言](../crates/core/src/config_tests.rs)，验证当前配置默认值和非法配置拒绝。

### 6. 移除历史序列化和哈希保护

证据：[WorkflowsConfig](../crates/core/src/workflow.rs) 仍通过 `default_parallel_steps` 等谓词
省略新增字段；[Workflow descriptor](../crates/storage/src/workflows/helpers.rs) 的 `version_digest`
仍只给 capability 2 加 `schemaVersion`，以保留 V1 编码。

复核更新：当前工作树中的 [SnapshotPolicyV1](../crates/service/src/capabilities.rs) 已改为始终
包含 workflows 字段；[对应测试](../crates/service/src/capabilities_tests.rs) 已改为检查当前配置。
这是已有未提交修改的源码观察，本次未执行其行为测试，不将其记作完成验收。

- [ ] 统一当前配置和 descriptor 的规范编码，删除仅为历史字节相等保留的省略/分支及
  [旧序列化断言](../crates/core/src/workflow_tests.rs)。
- [ ] 连同已有快照修改一起验证：当前配置与权限等有效输入变化能反映到摘要，规范编码可复现，
  篡改与错误身份被拒绝；不再要求旧开发版本的哈希保持不变。

### 7. 统一 Secrets 加密接口与 AAD

证据：[SecretCrypto](../crates/storage/src/crypto.rs) 保留 `encrypt/decrypt` 和
`encrypt_revision/decrypt_revision`，`associated_data` 根据 revision 是否存在选择 AAD 1/2。
当前 [部署写入](../crates/workers/src/pipeline.rs) 和
[RuntimeSource 读取](../crates/workers/src/runtime_source.rs) 使用 revision 绑定路径；
旧接口的仓库业务调用仅见于测试。

- [ ] 保留唯一的当前加解密入口与 revision 绑定 AAD，移除旧无 revision API、旧格式选择和重复实现。
- [ ] 将旧接口安全测试迁到当前入口，保留 nonce、key identity、account/worker/deployment/secret/revision
  绑定、密文篡改及错误上下文拒绝等覆盖；不读取或转换旧密文来维持开发数据兼容。

### 8. Runtime 包与测试流程收敛

已于 2026-08-29 完成本机实现与验收，见 [Runtime 包与测试流程整理](implemented/runtime-and-test-layout.md)
和[实测记录](implemented/runtime-and-test-layout-results.md)：workspace 690 个用例、90.16% 行覆盖率，
最终 23 目标各三轮全部通过。跨平台与正式发行资格另见[活动验收计划](runtime-layout-release-acceptance.md)。
以上是当次运行记录；后续[按用例分轮改造](implemented/test-repetition.md)已完成本机验收，采用完整一轮、时序补两轮，
现行验收规则以[测试节奏](references/testing.md)为准，不改写上述历史结果。

- [x] 将 `runtime/` 移至 `packages/runtime/`，生成目录改为 `dist/` 并取消 Git 跟踪；
  同步源码、配置、正式 pin、Rust 内嵌/manifest 消费者、workspace、CI 和发行构建路径。
- [x] 从不含 `dist/` 的干净源码快照构建并验证可复现资产，生产启动继续离线且不依赖 JS 工具链。
- [x] 删除已完成的 POC 上游能力探测、旧原型断言、重复测试及其孤立依赖；将仍需维护的产品
  回归与所需 harness、fixtures 统一移入 `test/`，保留当前安全、完整性和恢复覆盖。
- [x] 移除独立 POC pin、旧 G0 入口及相关必跑依赖；保留历史报告、已知限制和既有失败证据。
- [x] Gate 默认一轮，最终显式三轮；消除内部固定三轮、递归调用和重复 target，每轮各目标一次。
- [x] 减少重复构建、类型检查、初始化和无效等待；完整检查与 coverage 不随 Gate 轮次重复，
  记录实际执行次数与耗时，coverage 门槛仍为 90.00%。

## 执行与验收

先明确唯一 Workflow 模型，再配套收敛 schema、升级入口、descriptor 和测试；KV、Scheduler 配置、
Secrets 可按各自领域独立清理。每项都应同步修改生产调用方、fixtures、生成资产及当前文档。
旧设计文档中的兼容要求不凌驾于 Day1 约定；它们的历史结果也不应被重写为新实现的验证结果。

- [ ] 将保留旧行为的断言改为当前模型的成功、失败、安全、完整性和恢复测试。
  测试曾经通过不是保留旧实现的理由，也不能通过删除必要断言降低验收强度。
- [ ] 开发期间按 [测试节奏](references/testing.md) 运行相关单轮检查；完成实现与审查、源码冻结后，
  先执行 [AGENTS.md](../AGENTS.md) 要求的静态检查与一次 coverage，再统一执行完整 workspace
  一轮及登记时序用例的两轮追加，保留真实 workerd 路径和逐项执行证据。
- [ ] 修改 TypeScript 时通过严格类型检查、行为测试、构建和生成资产一致性检查；
  不手改 `packages/runtime/dist/` 生成产物。
- [ ] 完成记录写明实际修改范围、源码基线、执行的检查、结果与剩余项。
  未运行、缺少环境、只有源码修改或未满足对应逐用例轮数，都不能标为最终验收完成。

本次文档/策略变更仅需 `git diff --check` 与路径、来源和状态核对，不重跑 Rust 或 runtime Gate。
本清单不授权清空现有数据库、删除失败证据、下载 runtime、发布或执行需要提权的命令。
