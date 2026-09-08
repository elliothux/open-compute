# 代码质量提升专项

状态：TODO。本文跟踪不改变当前产品合同的源码结构、package 边界、生成物 ownership、
测试可维护性和仓库卫生改造。执行时以 Day 1 模型为唯一目标，不为旧路径、旧模块名、历史开发产物或
已退役 package 保留兼容层。

## 1. 扫描结论

2026-09-07 对当前工作树的静态扫描结果：

| 区域 | 规模／证据 | 结论 |
| --- | --- | --- |
| `crates/service` | `src/` 根约 147 个 Rust 文件，约 80 个顶层模块，15 个平铺文件超过 800 行 | 最高优先级，按业务领域和 composition ownership 收敛 |
| `crates/storage` | 约 71k 行，50 个根文件，存在 3,350 行 `workers.rs` 及 8 个超过 800 行的生产文件 | 最高优先级，在单一 SQLite/data authority crate 内按领域拆分 |
| `crates/artifacts` | 约 15k 行，27 个根文件、仅 3 个嵌套文件；`local.rs` 2,255 行 | 高优先级，收敛 Local、S3 client/preflight、R2 codec/multipart 和 cache |
| `crates/runtime` | 约 15k 行；`process.rs` 1,717 行、`supervisor/mod.rs` 1,301 行、crate tests 2,904 行 | 高优先级，拆分 process identity/group/I/O 与 supervisor state，保留一个权威生命周期 |
| `crates/workers` | 约 17k 行，41 个根文件；`pipeline.rs` 1,473 行、`runtime_source.rs` 1,109 行 | 高优先级，按 bundle/version/deployment、binding validation、routing/pins 和 runtime source 收敛 |
| `packages/runtime` | 约 16k 行，已按产品领域建目录，但多个 host/facade/transport 超过 500 行 | 保留目录模型，重点拆大文件和反向依赖 |
| `packages/toolchain` | 同时持有 Wrangler config projection、build/bundle、typegen、framework import 和薄 deploy wrapper | 保留一个 package，在内部按职责收敛；不再造部署协议 |
| `test/conformance` | `differential.ts` 1,081 行，`check.ts` 650 行，`adapters.ts` 581 行 | 中优先级，按 contract/product 拆分，但保留单一 inventory 和 Gate registry |

`crates/search`、`crates/document-parser`、`crates/images`、`packages/dashboard`、
`packages/cloudflare-extension`、`packages/docs`、`scripts/`、`examples/` 和 `share/` 当前边界清晰，
不做为了对称性的全面重排。

### 1.1 全仓简化审计清单

以下项目来自 2026-09-07 的第一轮 whole-repository simplification audit。它们只删除无消费者代码、
重复实现或多余工具链，不改变产品合同；实施前仍须用引用搜索和对应测试确认消费者集合。

- [ ] 删除已退役且无消费者的 `packages/operator-sdk/dist/` 本地 ignored 生成残留；它不是源码、
  发布输入或保留证据，不为该目录建立兼容入口；
- [ ] 将 `test/fuzz` 从带独立 `[workspace]`、`Cargo.lock` 和 `target/` 的第二套 Cargo universe
  收入根 workspace，共享 root dependency policy、lockfile 和 build cache；同步更新
  `docs/references/p1-fuzz-ownership.md`，继续由 `test/` 独立拥有 harness/corpus，而不是独立拥有工具链；
- [ ] 将 `crates/service/tests` 中重复的 S3 mock、artifact store、storage/runtime config 和 repo-root
  初始化收敛到现有 `tests/common`；只抽取字节等价的 fixture setup，不创建通用测试框架；
- [ ] 删除 Dashboard 中无消费者的 `StructuredSummary`、`CatalogFilters`、`DetailTabs`、query invalidation、
  deployment/hash/docs/format helper；若再次确认 `zod`、`date-fns` 没有其余消费者，同时移除这两个依赖；
- [ ] 合并 `storage` 中 workflow 与 scheduler workflow 重复的 row parse、token、digest 和错误构造 helper，
  仅共享语义一致的 codec，不统一两个领域不同的错误模型；
- [ ] 将 `crates/storage/build.rs` 的三组 migration checksum/constant generation 改为一份声明表和一条循环；
- [ ] 将 KV 与 D1 backup 中相同的文件 SHA-256 读取实现收敛到现有 backup helper，不引入新的 utility module；
- [ ] 删除只包裹 `MapEnv` 的 `StaticEnv` 和无消费者 re-export，测试直接使用 `MapEnv`；
- [ ] 删除 `artifacts/local.rs` 的本地 `MetadataExt` 转发 trait，直接使用标准库 Unix extension trait；
- [ ] 删除只转发 `ArtifactCache::sample_integrity()` 的 `sample_cache_integrity` 自由函数，公开并直接调用
  真正拥有该行为的方法；
- [ ] 删除 tracked Python bytecode
  `test/fuzz/corpus/document-parser/__pycache__/generate.cpython-314.pyc`，由通用 hygiene Gate 阻止复发。

第一轮估算可移除约 4,200 行 tracked source/lockfile、约 95,446 行 ignored 退役生成物和 2 个直接
Bun 依赖。数字用于排优先级，不是验收目标；Definition of Done 以消费者消失、唯一 authority 和检查通过为准。

### 1.2 第二轮简化审计清单

第二轮避开 1.1 已列项目，继续检查 production API surface、同领域 helper、CLI alias 和 UI 组件：

- [ ] 收窄 `open-compute-service` 的 public surface。它是 workspace 叶子和 binary composition root，当前却公开
  约 50 个 module 并重复 re-export 多个实现类型；生产 module 默认改为 private／`pub(crate)`，integration Gate
  只通过带 `test-support` 的最小入口访问，不把测试便利面伪装成产品 library API；
- [ ] 统一当前散落在 `service`、`storage` 和 `artifacts` 的 Unix millisecond 转换。复用
  `SchedulerClock` 的 infallible wall-time 语义，并在确实需要 domain-specific failure 的边界映射错误；不保留二十余个
  `SystemTime::now().duration_since(UNIX_EPOCH)` 本地 helper，也不把不同 fallback 语义静默合并；
- [ ] 在既有 Cloudflare v4 wire/storage 层保留一个 millisecond-to-RFC3339 formatter，删除 `accounts`、`vendor`、
  `queues`、consumer 和 Workers observability handler 中重复的 `jiff::Timestamp::from_millisecond` 包装；调用点只负责
  映射本领域错误；
- [ ] 删除 `oc run` 对 `wrangler deploy` 的纯命令别名，保留唯一的 `oc deploy` 名称并同步 README、用户文档和测试；
  若未来需要 watch/HMR，应直接设计成不同语义的 `dev`，不能继续复用一个含义错误的 alias；
- [ ] 合并 Dashboard 的 `ConfirmActionDialog` 与 `ConfirmDeleteResourceDialog`。保留一个具名确认组件和可选 force
  control，删除重复的 open/reset/form/input/error/button 状态机，不扩展成任意表单框架；
- [ ] 删除 `loader/wrappers/runtime.ts` 中重复声明的 `CacheRuntime`／`CacheRuntimeFactory`，使用
  `cache/facade.ts` 已拥有的类型合同；只使用 type-only import，不能制造运行时模块环；
- [ ] 删除仅供一个测试调用的 `exit_code(ExitClass)` pass-through，并统一直接的 `ExitClass` → `ExitCode`
  转换；如果实现 `From` 能减少当前重复表达且不扩大 API，则保留该唯一转换实现；
- [ ] 合并 `packages/toolchain` 的两份 canonical JSON 排序递归；`framework-output` 已依赖 `project`，应复用后者的
  validation/canonicalization primitive，而不是维护第二份对象排序规则。

第二轮的直接代码缩减量较小，静态估算约 150–250 行，但会显著减少 service 的意外 API 合同和时间处理分叉。
不以建立新的 `utils`、跨领域 facade 或通用 dialog framework 来换取这些删除量。

### 1.3 第三轮简化审计清单

第三轮面向全仓现有实现，避开 1.1、1.2 已记录项和当前 P11 工作树改动，继续删除零消费者
API、重复索引机械和已有标准能力的本地实现：

- [ ] 收敛 `crates/service/src/metrics.rs` 及 `metrics_*.rs` 的枚举索引机械：由枚举自身提供
  `ALL` 和 `index()`，删除重复的数组函数、`iter().position(...).unwrap()` 以及六份字节等价的
  success/failure outcome formatter；Queue 的 `error` 标签语义不同，不强行合并；
- [ ] 删除 `crates/storage/src/migrations.rs` 中 `migration_001_checksum()` 至
  `migration_011_checksum()` 的十一个单项 accessor；生产和测试统一从已有
  `migration_registry()` 读取有序 identity/checksum；
- [ ] 删除五条零直接消费者的 Cargo dependency 声明：`artifacts` 的 `thiserror`、`url`，
  `runtime` 的 `zeroize`，`service` 的 `aws-sdk-s3`、`zeroize`。这是直接 dependency edge 缩减；
  相同 package 仍可能由其他 crate 间接引入，不预先声称 lockfile 会缩减；
- [ ] 删除零消费者的 storage public API `check_workflow_create_capacity()` 和 `referrer_count()`；
  前者已有 `check_workflow_create_batch_capacity()` 作为唯一 capacity authority，后者当前没有产品调用点；
- [ ] 删除仅由 `open-compute-search` crate 内测试调用的 `exact_top_k()` 批处理 wrapper；
  生产和测试都直接使用已有 `ExactTopK` accumulator，不保留两套入口；
- [ ] 删除无消费者的 public constants：`XBERG_VERSION`、`XBERG_CRATE_SHA256`、
  `IMAGE_ENGINE_VERSION`、`MAX_VECTOR_ID_BYTES` 和 `MAX_NAMESPACE_BYTES`；保留 parser contract manifest
  和 storage authority 中真正参与验证的约束；
- [ ] 删除 runtime TypeScript 中无消费者的 `dateSetTime`、`workflowFailure()` 和
  `WorkflowActivation` 导出，不为未实现路径保留推测合同；
- [ ] 删除 `AiSearchCoordinator::run_startup()` 纯转发别名，唯一测试调用点直接使用
  `run_until_idle()`；
- [ ] 删除 `crates/storage/src/master_key.rs` 的手写 lowercase hex encoder，直接使用该 crate
  已依赖的 `hex::encode()`。

第三轮静态估算可减少约 230 行和 5 条直接 dependency edge。数字仅用于排序；实施时必须
重新确认消费者集合、指标序列顺序和固定 label 语义，不为追求行数引入新 dependency 或泛化框架。

## 2. Rust 源码按领域收敛

### 2.1 `service`

- [ ] 按 `instance`、`distribution`、`d1`、`kv`、`r2`、`ai_search`、`observability`、`metrics`、
  `runtime` 和 `operations` 等实际 ownership 收敛根目录文件；
- [ ] 保留 `cloudflare_v4/` 和 `workers_http/` 的薄 transport 边界，不把 domain workflow 塞进 handler；
- [ ] 拆分 `tests.rs`、`run.rs`、`metrics.rs`、`binding_backend.rs`、`runtime_bridge.rs`、`doctor.rs` 等超大文件。

### 2.2 `storage`

- [ ] 保留单一 `open-compute-storage` crate 作为 data-dir、SQLite、migration、identity 和 secret crypto authority；
- [ ] 将顶层 R2、queue、Durable Object、observability、snapshot/restore 文件收入已有领域目录；
- [ ] 按 catalog、persistence、transaction workflow 和 read model 拆分 `workers.rs`，不把 SQLite authority 拆成多个互相协调的 crate；
- [ ] 将 `tests.rs`、`scheduler_tests.rs` 和顶层 `*_tests.rs` 移到所属领域，保留跨领域事务测试的明确入口。

### 2.3 `artifacts`

- [ ] 建立 Local backend、S3 transport/credential/preflight、R2 model/codec/multipart 和 verified cache 的直接目录；
- [ ] 拆分 `local.rs`、`cache.rs`、`store.rs` 和 `client.rs`，不保留同一实现的旧顶层 module alias；
- [ ] `mock_s3.rs` 和 fixture binary 继续只在 test-support 边界可达，不因移目录进入生产 dependency graph。

### 2.4 `runtime`

- [ ] 将 `process.rs` 按 executable identity、process group/signal、bounded stdio 和 exit/reap 职责拆分；
- [ ] 将 `supervisor/mod.rs` 收窄为组合入口，状态转移只保留一份，不新建第二套 restart/recovery state machine；
- [ ] 按 process、supervisor、embedded payload、verification 拆分 crate 级巨型测试；
- [ ] `fsutil` 等安全边界只按已证明的 ownership 拆分，不用普通便利 API 取代 no-follow、mode、fsync 和 containment。

### 2.5 `workers`

- [ ] 收敛 bundle/descriptor/pipeline、resource lifecycle/pins、runtime source 和各 binding projection；
- [ ] 继续拆分 `pipeline.rs` 与 `runtime_source.rs`，避免 validation、persistence projection 和 artifact loading 重新混成一层；
- [ ] 将 workflow/queue/DO 的 lifecycle 与 crash tests 收到对应领域，保持 immutable Version/Deployment 和 routing pin 唯一实现。

### 2.6 明确不调整的 Rust 边界

- [ ] 复审但默认保留 `search`、`document-parser` 和 `images` 独立 crate：它们分别隔离搜索算法、
  重型/不受信文档依赖和图像 codec 依赖，属于真实 dependency/security boundary；
- [ ] `core` 保持小型基础概念平铺，只在一个概念出现多个真实子模块时建目录；
- [ ] 不新建 `common`、`utils` 或横跨所有 crate 的便利层；只把稳定、通用且真正属于基础合同的类型放入 `core`。

## 3. TypeScript runtime 与工具链

### 3.1 `packages/runtime`

- [ ] 拆分 `loader/host.ts`、`durable-objects/host.ts`、`loader/wrappers/runtime.ts`、`services/transport.ts`、
  `r2/facade.ts`、`kv/transport.ts` 和 `workflows/runner.ts` 等超大模块；
- [ ] 消除领域模块对 composition root `loader/host.ts` 的反向 import。当前 `loader/bindings.ts` 与
  `loader/modules.ts` 从 `host.ts` 导入 `bindingError`，而 `host.ts` 又导入它们；应直接依赖已有的
  `loader/shared.ts` 或所属 protocol，不保留经 host re-export 的隐式环；
- [ ] 定义并检查单向依赖：protocol/serialization → domain facade/transport → loader composition；
- [ ] 保留一个 RuntimeSource/loader 实现和一个 private binding transport，不因拆文件制造第二套 host bridge。

### 3.2 `packages/toolchain`

- [ ] 在 package 内部按 config projection、build/encode、type generation、framework import 和 Wrangler delegation 收敛；
- [ ] 保持上游 Wrangler 为唯一 deployment transport；`oc deploy` 等便利入口只能直接转发精确 pin 的
  Wrangler，不恢复自定义 API client、认证或 resource CRUD；
- [ ] 不把 Wrangler 完整 schema 复制成本地 model；本地 projection 只保留 build/typegen 确实需要的已验证字段。

### 3.3 其他 packages

- [ ] 将物理目录 `packages/types/` 直接改名为 `packages/workers-types/`，与
  `@open-compute/workers-types` 的 package identity 一致；同步所有 generator、conformance 和 lockfile 路径，
  不保留旧目录 alias；
- [ ] 保留 Dashboard、Cloudflare extension、runtime、toolchain、workers types 和 docs 的当前 package 边界；
  只有当两个 package 出现已证明的重复 authority 或错误依赖方向时才合并／拆分；
- [ ] package manifest 中的名称、路径、版本、职责描述和 private/publish 语义与真实 ownership 一致，
  不继承与 package 无关的通用描述。

## 4. Package、crate 与项目边界

- [ ] 将 Rust workspace 实际依赖 DAG 写入单一可检查规则：`document-parser` 无内部依赖，`search` 只依赖
  `core`，`images`/`artifacts`/`runtime` 只依赖它们声明的下层基础，`storage` 依赖 `core`/`search`，
  `workers` 依赖 `core`/`storage`/`artifacts`，`service` 是唯一 composition root；
- [ ] 补全 `test/check-boundaries.sh` 对 `search`、`images` 和 `document-parser` 的约束，不只检查原有五个 crate；
- [ ] 为 Bun workspace 增加同等的本地 package 依赖检查：Dashboard 可依赖 extension，runtime 不依赖 Dashboard/toolchain，
  workers types 只扩展固定上游 types，toolchain 不成为第二个 management client；
- [ ] 不为缩短 import 新建 package/crate；新边界必须能隔离重依赖、安全边界、生成/发布单元或唯一 authority；
- [ ] 将所有共享 production、build 和 dev dependency 版本收敛到 root Cargo workspace；清理当前散落在
  crate manifest 中的 `tempfile`、`tower`、`command-fds`、`signal-hook` 等版本声明；
- [ ] 增加 Rust 与 Bun 供应链检查，拒绝已知漏洞、未批准 license、非批准 registry/git source 和明确禁止的
  dependency；只约束有实际风险的传递依赖重复，不为了依赖树外观强行升级或替换上游库；
- [ ] 增加无效依赖和无效代码检查，识别没有 import 的 Cargo/Bun dependency、无消费者 feature、不可达 module、
  无消费者 re-export 和遗留 `dead_code` suppression；逐项确认后直接删除，不新建 facade 掩盖消费者缺失；
- [ ] 只保留一个 root Bun workspace、一个 `bun.lock`、一个 Cargo workspace 和一个 `Cargo.lock`。

## 5. 测试、合同与生成物

- [ ] 将 `test/conformance/differential.ts` 按产品或协议领域拆分，但保留一个 differential entrypoint、统一 credential
  policy 和一份结果汇总；
- [ ] 将 `check.ts`、`adapters.ts` 和 inventory expansion 按 schema、client adapter、evidence 与 generation 职责拆分；
- [ ] `test/gate.py` 保留为唯一入口，`test/gate_cases.py` 保留为唯一 case registry；只在能分离 discovery、
  scheduling、evidence/report 时抽出内部模块，不新增另一套 Gate runner；
- [ ] 梳理 OpenAPI subset、P6 capability source/generated capability、Workers types inventory 和
  `share/cloudflare-capabilities.json` 的生成链；每个产物必须明确标记 source authority 或 generated output，
  一条确定性命令负责生成，一条 check 负责拒绝 drift；
- [ ] 不把不同协议层的 schema 为了减少文件数强行合并，也不允许同一状态在多个手写 JSON/Markdown 中同时成为 authority；
- [ ] 将 property test 和有界 fuzz 扩展到 `compute.toml`、multipart Worker upload、R2/S3 header/codec、
  loader key、dashboard login/session token、instance descriptor、snapshot/restore manifest 以及 DO/Queue/Workflow
  持久化输入；断言 round-trip、canonicalization、size bound、拒绝非法输入和 panic-free，而不是只追求 case 数量；
- [ ] 保留一个受控 fuzz harness 和一份 corpus ownership；新的 seed/corpus 必须有来源、上限和目标 invariant，
  不为每个 parser 建立独立工具链或长期 fuzz service；
- [ ] 建立 `PlatformError`／`ErrorCode` 合同检查，确保每个错误码只有一个明确语义，HTTP、CLI、日志和持久化
  replay 映射一致，并拒绝 handler 临时拼接不稳定外部错误；
- [ ] 错误合同测试必须覆盖 4xx/5xx 分类、稳定响应 shape、重放语义和敏感信息清理，明确断言 path、token、
  SQL、内部 topology、upstream body 和原始异常不会进入外部响应或普通日志；
- [ ] 修复 coverage 插桩子进程的输出定位，所有 `*.profraw` 只能进入 `target/llvm-cov-target/` 或
  `.temp/gate-run/`，不再泄漏到 `crates/**`；根因修复后再清理非证据残留；
- [ ] 增加通用 repository hygiene 硬门，拒绝 tracked 或 misplaced 的 `*.profraw`、`__pycache__`、`*.pyc`、
  package `dist`、`.next`、`.wrangler`、非约定 Cargo target 和额外 lockfile；检查必须基于路径/产物类型，
  不为每次泄漏追加一次性文件名规则；
- [ ] 从 tracked source 移除当前 `test/fuzz/corpus/document-parser/__pycache__/generate.cpython-314.pyc`，并确保
  Python tooling 使用 `PYTHONDONTWRITEBYTECODE=1` 或将 disposable cache 定位到 `.temp/`；
- [ ] 已退役 `operator-sdk` 和其他历史 package 不得回到 tracked workspace。本地 ignored `dist` 残留与持久
  `.data/` 严格区分：前者可在明确的仓库卫生任务中清理，后者不得因重构删除或重置。

## 6. Rust 硬质量门

当前 workspace lint 只显式启用了少量定向规则，`unreachable_pub` 仍是 `warn`，`clippy.toml`
也只有 `large-error-threshold`。Canonical Clippy 命令中的 `-D warnings` 能阻止当次构建出现警告，
但不能检查单文件总行数，也没有把复杂度预算完整固化为仓库合同。当前共有 64 个维护中的
Rust 文件超过 800 行，其中约 36 个是生产源码；另有约 74 处
`#[allow(clippy::too_many_arguments)]`。硬门必须在这些存量问题直接重构完成后一次启用，不能用
grandfather allowlist、按路径豁免或提高阈值把存量合法化。

### 6.1 文件和函数规模

- [ ] 增加仓库级 Rust source-policy 检查，扫描 `crates/**/*.rs` 的维护源码，单文件最多 800 个
  物理行；`target/`、Cargo `OUT_DIR` 等生成目录和 `third_party/` 不在项目源码范围内；
- [ ] 生产、crate-local test、integration test、fixture binary 和 `build.rs` 使用同一 800 行上限，
  不给测试或特定目录永久豁免；
- [ ] 在 workspace Clippy 配置中将 `clippy::too_many_lines` 设为 `forbid`，并在 `clippy.toml`
  固定 `too-many-lines-threshold = 100`，函数体超过 100 行必须按职责拆分；
- [ ] source-policy 检查同时拒绝对 `clippy::too_many_lines` 的局部 `allow`／`expect`，确保硬上限
  不能从源码绕过；
- [ ] 检查失败必须输出文件或函数、实际行数和阈值，且接入常规 dependency/tooling check 与
  最终 Gate，不能只作为人工 review 提示。

### 6.2 提升现有 lint 基线

- [ ] 将 `unreachable_pub` 从 `warn` 提升为 `deny`；逐项启用高信号规则，不一次开启完整
  `pedantic`、`nursery` 或 `restriction` lint group；
- [ ] 将非跨 crate API 收窄为 private 或 `pub(crate)`，并增加 workspace public API surface snapshot/check，
  用于发现意外暴露和跨层依赖；该 snapshot 不是历史 semver 合同，不得据此保留 Day 1 已废弃 API；
- [ ] 显式 `deny` `clippy::too_many_arguments` 和 `clippy::type_complexity`，移除现有无理由
  `#[allow]`；真实外部协议边界如确需例外，只能使用带 `reason` 的窄范围 `#[expect]`，不能借参数
  对象制造新的无行为抽象；
- [ ] 启用 `clippy::allow_attributes_without_reason`，所有保留的 lint 例外必须说明不可直接消除的
  correctness、协议或性能原因，并由对应测试覆盖；
- [ ] 对生产 target 启用 `clippy::unwrap_used`、`clippy::expect_used`、`clippy::panic`、
  `clippy::todo` 和 `clippy::unimplemented`；测试代码可使用有诊断价值的 `expect`，但生产路径只能
  传播或在语义边界转换错误，编译期不变量必须用带理由的窄范围 `expect`；
- [ ] 评估并固定 `clippy::large_futures`、`clippy::large_stack_arrays` 和
  `clippy::large_stack_frames` 的显式阈值。阈值由当前支持路径的测量结果和单机运行时预算决定，
  不以“让现有代码通过”为准；
- [ ] 每个新 lint 先修完它发现的全部问题，再进入 workspace 配置并由 canonical Clippy 命令执行；
  不提交长期 warn-only 过渡态或基线 suppression 文件。

## 7. 实施顺序

1. **CQ1 边界与 lint 合同固定**：补全 Rust/TypeScript dependency 和供应链检查，固定 800/100 行硬阈值，
   生成依赖、API、lint 现状清单并确定 source/generated authority；
2. **CQ2 反向依赖**：先消除 `packages/runtime` 的 composition-root import 环和已证明的重复 authority；
3. **CQ3 Rust 领域收敛**：按 storage、artifacts、runtime、workers、service 的独立变更批次执行，
   同时清零文件／函数长度和新增 lint 的存量问题；
4. **CQ4 工具与测试**：整理 toolchain、workers-types 路径、conformance 和 Gate 内部结构，扩展 property/fuzz
   与统一错误合同检查；
5. **CQ5 硬门启用**：在无豁免通过后，将 source-policy 和全部选定 lint 接入 canonical checks；
6. **CQ6 卫生与验收**：修复生成物泄漏，删除已证明的退役残留，完成最终单轮验收。

每个批次必须是可独立 review 的直接改造；不把整个项目一次性搬家，不在纯移动中夹带新功能或持久化变更。

## 8. 统一改造约束

- 目录、crate 和 package 必须表达真实 ownership，不为文件数量对称创建无行为 facade、通用 trait 或 manager 层；
- 直接更新 `mod`、import、manifest、generator 和调用点，不保留 `#[path]` 跳转、旧模块 alias、双重导出或旧 package 转发层；
- 删除被新结构取代的文件、参数、re-export、fixture 和检查路径，不留“以后再删”的过渡实现；
- 保持 security boundary、数据完整性、crash/restart recovery、immutable deployment、单进程 ownership 和固定 workerd 合同；
- 拆分以 ownership 和行为边界为准，同时满足文件 800 行、函数 100 行的硬上限；不得为了过门
  机械切出无行为 helper、pass-through module 或任意分片，协议矩阵应按场景／阶段提取具名 fixture 和断言。

## 9. Definition of Done

- 高优先级目录不再依赖大量顶层文件名前缀表达领域，且巨型文件已按真实 ownership 拆分；
- Rust 与 Bun workspace 依赖方向均有机器检查，composition root 不再被下层模块反向依赖；
- 共享依赖版本只有 root authority，供应链 policy、无效依赖／代码检查和最小 public API surface 检查通过；
- 每个协议、schema、inventory、runtime source 和持久化模型都只有一个可识别 authority，生成物可重现且 drift check 通过；
- property/fuzz 覆盖关键不受信边界和 canonicalization invariant，错误码及其 HTTP/CLI/log/replay 映射一致且不泄密；
- 未引入循环依赖、冗余 re-export、无效抽象、兼容 shim 或产品行为变化；
- 所有维护中的 Rust 文件不超过 800 行、函数不超过 100 行，无长度 lint 局部豁免；新增高信号
  lint 已清零存量并成为 workspace 硬门；
- 没有源码树 coverage/build 残留，也没有删除未获授权的 `.data/` 或保留失败证据；
- 按仓库政策通过 format、Clippy、no-default-features、MSRV、metadata、Rust/TypeScript 依赖边界、
  TypeScript build/typecheck、coverage 和最终单轮 workspace Gate。
