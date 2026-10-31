# R2：架构与代码审查收敛

状态：**implemented（2026-09-30）**。R2 直接收敛 crate ownership、resource lifecycle、HTTP composition、Dashboard 和 public API surface；不改变已声明的 Cloudflare 行为、持久化语义或单 daemon 部署模型，也不保留兼容别名。

## 用户结果

- 每种资源只有一套生命周期编排；Cloudflare transport 只校验 wire 输入、调用 product workflow 并映射响应。
- `service` 继续是唯一 composition root，但不拥有本应位于 product/controller 层的状态机。
- Dashboard 只展示当前支持的产品，所有生产 API 类型、错误和 query identity 都从 `@open-compute/sdk` 与一个本地 query authority 取得。
- storage、workers、search 和 images 的职责从 module path、公开面和依赖方向即可辨认，不再依赖重复实现或模糊命名。
- 零消费者 production API 被删除；测试钩子只在 `cfg(test)` 或 `test-support` 下可达。

## 已确认的审查结论与修复

### 1. R2 生命周期与 transport 分层

`crates/service/src/cloudflare_v4/r2.rs` 当前在 handler 内执行 `reserve_create`、异步 reconcile、`mark_ready` 和 `complete_create`，而同步产品由 `open_compute_workers::ResourceController` 统一拥有相同生命周期。这是已确认的第二套状态机，也是本轮唯一明确的 `service` 错层。

将 R2 create/reconcile/delete 的 durable workflow 收入 `open-compute-workers` 的 R2 ownership，保留一个 R2 专用 async controller；不为了单个 async driver 把全部 `ResourceDriver` 改成通用 async plugin framework。controller 持久化与 wire 无关的 replay result，Cloudflare handler 只负责请求校验、Cloudflare envelope 和稳定错误映射。启动恢复和 maintenance 必须调用同一 controller primitive，不再各自拼接状态转换。

### 2. 单一 identity 与 limit authority

- `storage/src/workers.rs` 和 `workers/src/pipeline/runtime_features.rs` 的 `idempotency_ref_id` 字节级相同。保留 storage 中与持久化 referrer identity 同属一个 authority 的实现，向 workers 暴露该函数并删除副本；前缀、字段顺序和编码由一组固定向量测试冻结。
- `storage/src/vectorize/engine.rs` 不再本地声明 10 KiB metadata 上限，直接使用 `open_compute_search::MAX_METADATA_BYTES`；不同协议的 R2、Cache、release 等同名上限继续各自拥有。
- `service/src/document_parser_backend.rs` 不再直接调用 `image` codec 实现 VLM JPEG resize。把 bounded decode/resize/encode primitive 放入现有 `open-compute-images`，service 继续拥有 VLM contract、deadline 和 provider workflow。`document-parser` 的 OCR 预处理边界不变。

### 3. HTTP composition 与 `HttpState`

`admin_router` 和 `merged_router` 当前重复 health、`/client/v4`、metrics、operator session/surface 和 test control 组合。提取一个直接的 admin route builder，public/merged 各自只增加它们独有的 artifact、local ingress、host-first 和 middleware 行为；不新建 router framework。

`HttpState` 当前以大量独立 `Option<Arc<*ApiState>>` 同时表示构建中状态、测试裁剪和 listener capability。按实际 listener/product ownership 收成少量具名 state bundle，并在 listener bind 前验证其必需成员；保留 Axum 组合所需的共享 state，不引入 DI container、service locator 或另一套 runtime registry。

### 4. Dashboard ownership

- 将 Workers、D1 和 Workflows detail route 中的 dialog、form、tab content 和 mutation interaction 移到各自 `components/` 或 `features/<domain>/`；route 只保留 loader/query composition、URL state 和页面布局。只提取有明确职责的组件，不按行数制造 wrapper。
- 扩展现有 `lib/query-options.ts`，由按产品命名的 query key factory 同时服务 list、detail、prefetch 和 invalidation。删除页面内手写的 `cloudflare-v4` key，不建立通用 data layer。
- Dashboard production source 的 `APIError` 和受支持资源类型只从 `@open-compute/sdk` 导入；SDK 缺少的受支持类型由 SDK 明确 re-export。E2E 为验证官方 Cloudflare client 兼容性而直接使用 `cloudflare` 的 case 可以保留。
- 将 `lib/cloudflare.ts` 改名为 `lib/management-client.ts`，名称直接表达它只是 capability-scoped SDK 的 browser composition；同步删除旧路径，不留 re-export alias。
- 删除 Browser Run、Containers 和 Sandbox 的导航项、占位 route 与“已支持产品”展示。以后只有在对应能力进入声明支持面时再添加，不保留 future scaffolding。
- 侧栏与 account home 如果继续展示相同产品 metadata，复用一个 supported-product catalog；operator-only 页面和不同用途的分组不强行合并。

### 5. storage 公开面与命名

将 `open_compute_storage::workers` 改为能表达 SQL authority 的 `worker_repository`。同步更新 crate 内路径和所有消费者，删除旧 module path；`open-compute-workers` 继续表示 bundle、deployment、resource lifecycle 和 routing orchestration。

移除 `storage/src/lib.rs` 的领域类型平铺 re-export。调用方从 `d1`、`kv`、`r2`、`resources`、`worker_repository` 等 owning module 导入；crate 根只保留 `PlatformStorage`、data-dir/control-db 等真正的 crate 入口。该变更一次性更新消费者，不保留 root alias 或双路径。

### 6. public surface 与死 API

重新以 production、crate-local tests、integration Gates 三类消费者核对 `open-compute-service` 公开面：

- 删除当前零消费者的 `D1BindingService::operator_list_tables` 和 bounded `RuntimeTransport::repair_alarm`；
- `SchedulerService::pause_kind`／`resume_kind`、`D1BindingService::operator_query`、`SqliteKvBindingExecutor::with_connection_limit`、`ServiceInvocationRegistry::clear_generation` 仅有测试消费者，改为 `cfg(test)` 或 `test-support` 下的最小可见入口；
- bounded `RuntimeTransport::dispatch_alarm` 被真实 integration Gate 使用，保留但只在 `test-support` 下公开；production scheduler 继续使用内部 unbounded primitive 和自己的 deadline；
- `QueueEnqueueHold::{block_before,release_before,block_after,release_after,wait_seen}` 已正确受 `cfg(any(test, feature = "test-support"))` 保护且被 crash Gate 使用，不删除。

### 7. 文档与 package identity

- root `AGENTS.md` 的 architecture ownership 补充 `search`、`document-parser` 和 `images` 三个叶子 crate，与 `test/check-boundaries.sh` 的实际 DAG 一致。
- `open-compute-core` Cargo description 删除不准确的 “Dependency-free”，改为基础 config、error、ID、secret、health 与 clock 职责；它没有 workspace-internal dependency 不等于没有 dependency。
- R2 完成后同步本页、`docs/README.md` 和受影响的持续维护文档；不把历史 review 当成新的兼容合同。

## 实施证据（2026-09-30）

- `bun run build`、Dashboard TypeScript/tests/build、Rust format/Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundaries 与 source policy 均通过。
- `./test/coverage.sh --jobs 2` 的 54 项 workspace Gate 全部通过，line coverage 为 **90.08%**（最低要求 90.00%）；报告保存在 `target/llvm-cov/{html,lcov.info,summary.json}`。
- source freeze 后唯一一次最终 `./test/gate.py --workspace --jobs 2` 通过 54/54 目标；报告为 `.temp/gate-run/20260930T192920-fd616dea/report.json`。
