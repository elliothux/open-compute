# P0.3：资源与 Binding

> 状态：已实现并通过 Exit Gate（2026-08-25）

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- 资源 catalog 和 lifecycle 由 control authority 管理，产品数据留在其拥有的存储。
- Binding 在部署时验证 account、资源 kind、身份与权限，随后冻结到不可变部署描述；不能从请求输入重新选择 scope。
- 调用由 ResourcePin 保持资源存活；删除先阻止新引用，再等待／协调现存引用并完成受控回收。
- RuntimeSource 每次冷／热请求都验证部署与资源状态，不能用宿主内存 registry 替代 SQLite authority。
- Private BindingBackend 验证 generation capability 和 binding 身份，在边界处理有界 frame／stream 与稳定错误。
- 资源创建、删除、staging、文件身份和跨库状态通过持久化 operation 与 reconciliation 收敛；失败不会被静默修复成成功。

## 源码入口

- [`crates/workers/src/resource_lifecycle.rs`](../../crates/workers/src/resource_lifecycle.rs)
- [`crates/workers/src/resource_pins.rs`](../../crates/workers/src/resource_pins.rs)
- [`crates/storage/src/resources.rs`](../../crates/storage/src/resources.rs)
- [`crates/storage/src/bindings.rs`](../../crates/storage/src/bindings.rs)
- [`crates/service/src/binding_backend.rs`](../../crates/service/src/binding_backend.rs)

## 验收依据

以下保留原阶段验证记录，命令与轮数不作为当前执行要求：

已实现的 production framework 包括：UUIDv7 resource/binding identity、`003` forward-only
migration、resource/binding/referrer authority、create/delete reconciliation、typed canonical
descriptor、deployment hash 与 warm-load invariant、静态 loader factory、独立 backend token、
每次调用的 DB authorization/resource pin、固定 byte/time budget、稳定错误、低基数 metrics 和
secret-free doctor inspection。P0.3 的 production KV executor 按阶段边界保持 fail closed；真实
KV 数据引擎仍属于 P0.4。

已验证证据：

- `./test/test-p0-3.sh`：RB-01 至 RB-18 连续三轮 fresh-process 全部通过，并继续跑通三轮 P0.2
  regression Gate；
- `./test/coverage.sh`：workspace 全目标、全 feature 测试通过，Rust 行覆盖率 90.04%，不低于
  90.00% 门槛；
- format、Clippy、no-default-features、Rust 1.98 MSRV、metadata、dependency boundary 与 diff
  whitespace 检查通过。

当前测试入口与规则见[测试手册](../references/testing.md)。
