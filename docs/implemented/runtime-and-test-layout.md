# Runtime 包与测试流程整理

记录日期：2026-08-28。

状态：2026-08-29 本机实现与验收完成；跨平台、特权 egress 和正式发行资格另由
[发行验收计划](../acceptance/runtime-layout-release-acceptance.md)跟踪。当前命令与并发规则见
[测试节奏](../references/testing.md)，实际测量、失败修正和三轮证据见[实测记录](runtime-and-test-layout-results.md)。

## 实现

- TypeScript runtime 源码与行为测试按 domain 归入 `packages/runtime/`，生成的 `dist/` 不提交。
- Build manifest 固定源码、配置、依赖和生成模块集合；Cargo 只消费验证后的资产，生产启动离线。
- 构建工具保留源码目录结构，并验证独立输出的字节和完整文件集合一致。
- 测试入口统一为 `test/gate.py`；当前调度、隔离与轮数规则只维护在[测试手册](../references/testing.md)。

## POC 删除与断言归属

以下分类覆盖原 `poc/` 的全部源码/fixture。未保留第二个宿主、静态部署注册表、模拟绑定协议、
pin、下载器、Reporter 或旧 G0 命令。`docs/implemented/g0-results.md` 保持原始字节；保留
历史 `.temp/g0-run/failed/`。不要求重建历史报告，也不把历史 case 数量当成当前验收要求。

P0.7 的已有生命周期矩阵从 991 行缩至 810 行；保留在一个集成测试中以共享同一真实部署的
promotion/rollback/delete 上下文，新恢复断言和 Worker fixture 已按职责提取，不继续扩张该文件。

| 退役测试/断言组 | 分类和现行证据所有者 |
| --- | --- |
| bootstrap lock/checksum/config/invalid-config | 已完成的上游启动调查；产品验证由 `crates/runtime/src/tests.rs` 与 `crates/runtime/tests/supervisor.rs` 承担 |
| bootstrap port collision/unwritable-dir/readiness/internal paths/exception | 等价产品覆盖：P0.1 端口、数据目录、两阶段 readiness；P0.2/P0 Exit 内部路径拒绝、稳定错误 |
| bootstrap TERM/KILL/restart/harness-exit/no-leaked-child | 等价产品覆盖：supervisor、P0.1 orphan/reap、P1 crash process |
| loader L01/L02/L03/L04/L05/L07/restart | 等价覆盖：P0.2 cold/warm、A/B coexist、promote/rollback、100 concurrent cold requests、重启 credential 轮换 |
| loader L06/entrypoints/body/stream/identity/request-id/active-route | 等价覆盖：P0.2 与其 `http.rs`，真实持久化 deployment、非法 bundle、命名入口、上传取消、流中断、伪造身份剥离 |
| loader L08 outbound | 等价覆盖：P0.2 private/loopback/metadata 拒绝及 Linux dual-stack/DNS/redirect 网络夹具 |
| loader key reuse/sanitized errors | 等价覆盖：workers descriptor/pipeline/runtime_source 单测、P0.2 immutable identity；P1 security、P0 Exit 与 Gate 输出 canary 检查 |
| loader unknown-kind、scheduled/queue/workflow-unimplemented | 旧 POC dispatch envelope/未实现分支已经过时；当前 Queue/Cron/Workflow 由 P2 产品 Gate 直接验证 |
| loader D-abort | 已完成且保留限制的调查。不能保证客户端断开取消 isolate；P0.2 继续验证上传停止和 workerd 崩溃后的流中断，不放宽失败形状 |
| binding cold/warm、B01/B02、path-url-as-data、capability surface | 等价覆盖：P0.3 资源作用域和 capability、P0.4 KV、P0.5 R2、P0.6 D1、P1 两账户隔离；旧 fake binding host 删除 |
| binding B03、structured-clone、F4/F5、fault isolation/unbound worker/logs | 模拟 JSRPC 后端的可行性/失败窗口调查已完成；产品 facade/RPC/错误/事务覆盖归属 P0.3–P0.6、runtime 包行为测试和 P0 Exit。模拟协议不作为产品实现保留 |
| DO D01/D02/D03/D04/D05、facet/class/storage/JS-state isolation、identity/invalid input/logs | native facet 可行性调查已完成；P0.7 与 storage/workers DO 单测验证当前 namespace/class/object identity、RPC/fetch、SQL/async transaction rollback、delete/purge；P0 Exit 验证跨产品隔离 |
| DO concurrency-no-lost-update | 保留并补入 `test/runtime/durable-objects/recovery.rs`，复用 P0.7 真实生产宿主及持久化 authority；不复制 POC fixture registry |
| recovery D06 restart/second object/supervisor/fresh-dir/unwritable-dir | 等价覆盖：P0.1、P0.7、P0 Exit、P1 crash process 和 single-binary 的 clean-init/restart/orphan 路径 |
| recovery seeded crash-loop、rollback、F6 pre-commit、F8 idle、F9 concurrent SIGKILL | native-facet 的具体故障窗口调查已完成；现行产品保留事务回滚、重启持久性、confirmed in-flight SIGKILL 与恢复后可用性。保留项由 P0.7 + `test/runtime/durable-objects/recovery.rs` 验证，先观察状态再 kill，不保留旧 fixed-sleep probe |
| recovery F7 write-confirmed response failure | 保留并迁入上述 recovery 模块：真实 DO 同步提交后响应失败，当前与 SIGKILL 后读取一致，不把请求失败当成未提交 |
| recovery F10/F11 native abort/get window | 已完成的一次性原生 facet 调查；不重跑旧窗口，也不保持原型动态代码选择 |
| recovery D07/D08/D09 promotion/rollback/delete | 等价覆盖：P0.7 generation fencing、in-flight promotion、rollback、显式 delete 和 worker 删除后 purge；P0 Exit snapshot/restore |
| 各 suite no-leaked-workerd-child | 现行 supervisor/P0.1/P1 crash/单文件和各 Gate shutdown/owner-registry 断言承担；不保留旧 harness 清理器 |
| `poc/tests/all.js`、各 Reporter、自测、orphan-helper、`poc/g0`、package、README | 仅服务退役调查/报告的调度与自检代码删除；新调度行为由 `test/test_gate.py` 验证，无历史 allowlist |
| `poc/harness/**`、`poc/workerd/**`、`poc/workerd.lock`、`poc/fixtures/**` | 仅服务上表退役实现；删除。存续恢复行为直接接入 P0.7，其唯一 fixture 位于 `test/runtime/fixtures/durable-objects/counter.js` |

以上是覆盖所有权说明，不是一次新的测试通过报告。未改动旧开发 schema/Workflow 双引擎等
独立 Day1 清理范围；它们随后已由归档的 [清理记录](day1-architecture-cleanup.md) 完成。

## 性能测量与验收

实际构建、串行／并行测量、失败诊断、coverage 和最终 Gate 完整保留在[实测记录](runtime-and-test-layout-results.md)。
该报告中的三轮要求是历史执行事实；当前执行方式以测试手册为准。未完成资格见[验收索引](../acceptance/README.md)。
