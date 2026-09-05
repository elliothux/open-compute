# P0.4：KV

> 状态：Implemented and verified（2026-08-25）

已完成阶段的维护摘要；当前支持范围见[兼容矩阵](../references/cloudflare-compatibility.md)。

## 实现与不变量

- 每个 namespace 拥有独立 SQLite 数据库，catalog 与资源生命周期由控制库管理。
- Key 使用 UTF-8 字节序，不做 Unicode normalization；值与 metadata 在 authority 边界验证。
- `put` 是单事务原子替换；失败保留旧值，事务内不执行异步网络或文件工作。
- 过期记录在读取和 list 时立即不可见，后台 GC 只回收空间。
- 分页 cursor 绑定 namespace／查询 scope 并验证完整性；不能用跨租户或被篡改 cursor 读取数据。
- 大值使用有界 streaming，连接、事务、资源 pin 与取消路径共同管理生命周期。
- 在线 backup 产生一致 SQLite 副本；restore 到新资源，校验数据库与文件身份后才发布。损坏隔离到单 namespace。

## 源码入口

- [`crates/storage/src/kv`](../../crates/storage/src/kv)
- [`crates/workers/src/kv.rs`](../../crates/workers/src/kv.rs)
- [`crates/service/src/kv_backend.rs`](../../crates/service/src/kv_backend.rs)
- [`packages/runtime/src/kv`](../../packages/runtime/src/kv)

## 验收依据

以下保留原阶段验证记录，命令与轮数不作为当前执行要求：

2026-08-25 验证记录：

- `cargo fmt --all --check`、workspace clippy（`-D warnings`）、no-default-features、Rust 1.98 MSRV、
  metadata、dependency boundaries、`git diff --check` 全部通过；
- `cargo test --workspace --all-targets --all-features -- --test-threads=1` 全部通过；
- `./test/test-p0-4.sh`：P0.4、P0.3、P0.2 各三轮 fresh process 全部通过；
- `./test/coverage.sh` 从 clean coverage state 运行全部测试和真实 P0.1–P0.4 Gate，
  `22166 / 24612` production Rust lines covered，90.06%。

当前测试入口与规则见[测试手册](../references/testing.md)。
