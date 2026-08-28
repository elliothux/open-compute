# P2.4 Workflow Core 验证记录

> 状态：最终验收通过，P2.4 Conditional Go。新增 Workflow 支持面的唯一条件是 DO 内 create 因 output-gate 不支持而 fail closed；read-only get/status 可用。
>
> 基线 revision：`95c389efde56b93e8ed329a0467390fa132dfb37`，加当前 P2.4 worktree。

## 实现范围

生产路径使用 control migration 012 与 scheduler migration 005；definition/version/binding 与 live referrer 由 control 管理，run/step/result 由 scheduler 管理。实例创建先保留 ref，再写入 scheduler，最后 finalize；旧实例永久冻结原 deployment/class。顺序 `step.do` 的成功结果在 SQLite commit 后才返回；重放、generation/run/step token、lease、quota 与错误分类均由生产 backend 校验。

`WorkflowEntrypoint` 由 stock workerd 的 dynamic loader 执行，callback facade 留在同一 tenant realm 的闭包中；原始 run/step token 只留在 system isolate 的 request-scoped `RpcTarget` controller。它只返回无令牌的执行/replay verdict，并在 RPC 完成后销毁；SQLite 仍是 grant authority。内部 transport 不进入 tenant env 或 public event。无 Workflow binding 的旧 wrapper V1 保持不变，避免破坏既有 WorkerCode descriptor digest。

支持 `create`、`get`、`id`、`status`、两参数顺序 `step.do`。不实现 retry、durable sleep、event、parallel、modifier 或 retention。独立 DO output-gate probe 对当前 pin 为 no-go，因此 DO 内 create 固定拒绝，read-only get/status 可用；这是唯一预期 Conditional-Go surface。

新增 Workflow 的 config/JSON、control/scheduler repository、backend/HTTP、dispatcher、metrics/doctor 与测试分别放在其 owner 的独立模块中；同时抽出原 deployment binding/validation 和 loaded-isolate module assembly。既有超过 800 行的配置、启动 composition、共享 HTTP transport 与 P0/P1 集成测试矩阵本轮仍保留原组织：这里只接入同一 authority/连接 owner、配置字段和迁移断言，新的 Workflow 状态机不放入这些大文件。共享 transport 保持一个 generation/auth/streaming 生命周期，不为 Workflow 再建并行实现。

## 固定输入

- workerd：`v1.20260826.1`，版本输出 `workerd 2026-08-26`；不下载、不改 pin。
- lock SHA-256：`d3614a6394cf85e24704954d9e7a9585fb38a2107e8eb73a02519e39add14d2e`。
- 本机 darwin-arm64 binary SHA-256：`2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403`。
- 配置与 system worker 源：`runtime/config.capnp`、`runtime/system-workers/`。Gate 使用生产 compiler 生成每代配置。
- Gate：`crates/service/tests/p2_4_workflow_hard_gate.rs`、`p2_4_workflow_product_gate.rs` 及 `workflow_support/`。
- JSON 共享 fixture：`runtime/tests/fixtures/workflow-json.json`；Rust 与 Node 对同一组数据校验。

本轮验证平台为 darwin-arm64；未执行 Linux-only sudo egress 专项，也未进行发布打包、部署或运行时下载。

## 验证入口

开发、审查和修复期间仅运行相关单轮；实现收尾、源码冻结后才做最终三轮验收。见
[Gate 验证节奏](../references/testing.md)。

```sh
OPEN_COMPUTE_GATE_ROUNDS=1 \
OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260826.1/workerd" \
  ./scripts/test-p2-4.sh
node --test runtime/tests/workflow-json.test.mjs
```

最终验收使用 `OPEN_COMPUTE_GATE_ROUNDS=3`（脚本默认值也是三轮），再运行相关 aggregate 与
`./scripts/coverage.sh`。缺少 binary 或校验失败不会跳过。失败现场保存在 `.p2-4-run/failed/`，
单轮开发结果不能替代最终三轮 aggregate evidence；下表保留本次实际完成的轮数。

| 检查 | 已观察结果 |
| --- | --- |
| `./scripts/test-p2-4.sh` | 最新版 PASS：三轮各 2 Hard + 6 Product，随后 core 4 / storage 16 / workers 9 / service 9 个 Workflow 单元测试通过 |
| `cargo fmt --all --check` | PASS |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| `cargo +1.98.0 check --workspace --all-targets` | PASS |
| `cargo metadata --no-deps --format-version 1` | PASS |
| `./scripts/check-boundaries.sh` | PASS |
| `cargo test --workspace --all-targets --all-features -- --test-threads=1` | 最新版完整 PASS；包含 service 147 / storage 161 / workers 52 项，以及全部 P0/P1/P2 集成测试 |
| `./scripts/test-p2-3.sh` | 最新版 PASS：三轮 fresh-process 及附加持久化、迁移、生产构建检查 |
| `./scripts/test-p2-2.sh` | 最终 aggregate PASS：三轮，递归包含 P2.1、P1、P0 与 G0 |
| `./poc/g0 test all` | 在上述 aggregate 内 PASS：三轮 Conditional Go；唯一非 PASS 仍是精确 `loader:D-abort`，各轮 `abortEvents 0 -> 0` |
| `./scripts/coverage.sh` | 最新版完整 PASS：**90.12%**（52,423 / 58,170）；门槛保持 90.00% |
| `node --test runtime/tests/workflow-json.test.mjs` | 最终 PASS：3 tests，0 failed / skipped |

三轮 Product Gate 已验证完整生产 facade、1,024/1,025 steps、1 MiB result 边界、版本冻结、workerd restart、platformd SIGKILL、P1 双库快照/fresh-host replay、S3 cache eviction/refetch、跨产品 at-least-once 与幂等 key、四池公平调度。GC 用例让 V1/V2 属于同一 Worker：活跃 V1 instance 阻止旧 deployment 删除，terminal release 后实际回收旧 S3 artifact，历史 status 仍可查询。夹具回收测试确认 supervisor stopped、owner registry 为空且子进程已 reaped。

生产 HTTP 矩阵覆盖 claim/success/failure × commit 前后 × Known/Unknown 共 12 组；每个独立 supervisor 再验证 create 的四种对应组合。故障只切断测试端的真实 HTTP 请求/响应，正常路径调用 `WorkflowBindingService` 与生产 SQLite repository，没有替换持久化引擎。十个真实子进程 SIGKILL 边界与事务内 SQLite abort 覆盖见下表。

release metadata 的 conformance 标识为 `go-p2.4-workflow-core-v1`。最终静态检查、完整 workspace、相关 P0/P1/P2/G0 aggregate 与 coverage 均已退出成功；验收结论为上述精确支持面的 Conditional Go，不扩大既有 G0 限制。

追加安全回归发现：tenant 修改 `Promise.prototype.constructor/then` 后，可以观察同 realm 异步返回中的私有 step grant；只用闭包隐藏变量不足以隔离令牌。Hard Gate 已先复现 `observedPrivateGrant=true`，随后将原始令牌收敛到 system isolate controller；最新三轮 Hard Gate 和真实 production-backend Product 测试均返回 false。修复后的完整 workspace、aggregate 和最终 coverage 均通过。[RPC 可见性](https://developers.cloudflare.com/workers/runtime-apis/rpc/visibility/)与[RPC 生命周期](https://developers.cloudflare.com/workers/runtime-apis/rpc/lifecycle/)是 controller ownership 的上游依据。

DO caller 传输失败已按原测试顺序复现：第 0 组 status 返回后，下一次 create 遇到 HTTP incomplete-message；不是 connect/timeout，workerd 始终 Running/Ready。单独运行的 256 组 unread POST 曾通过，因此不能据此排除连接交接竞态。仅发送 `Connection: close` 的尝试虽然通过一次定向运行，但完整 Gate 仍失败；本地 Hyper 的请求编码与 workerd/KJ 响应路径表明这个 header 并未可靠阻止复用，已删除该无效处理。

当前 `WorkerdTransport` 直接为带 body 的 tenant dispatch 使用禁用连接池的 HTTP client；空 body 与已读取输入的 custom-event 协议仍使用原连接池，两者共享同一 generation/auth/supervisor owner。不缓存输入、不重试 mutation、不添加内部 header 或失败 allowlist。新增单测检查实际 TCP peer 不复用，并验证未就绪输入流不会阻塞响应；该单测、修复后的 P2.4 三轮和完整 aggregate 均通过，三轮 Product 分别耗时 178.66s / 189.10s / 196.19s。KJ HTTP 的[未消费 body 关闭路径](https://github.com/capnproto/capnproto/blob/master/c%2B%2B/src/kj/compat/http.c%2B%2B)解释了为什么前一响应完成不等于后续请求可安全复用；本记录的实际失败与通过结论仍以 pinned binary 的测试为准。

保留的失败记录：早期三轮尝试曾在第二轮 `workerd --version` probe 触发既定 10s timeout；首次覆盖率运行曾在 P0.1 第二轮重启后 ready 等待超时（退出 101），诊断保留于 `.p0-1-run/failed/1787825379714287000/`。后续三轮 P2.4 和完整覆盖率均已退出成功；未放宽校验时限、接受条件或 coverage 门槛，也未删除失败现场。

最终 coverage 中，Workflow Hard 2 项与 Product 6 项再次通过，Product 用时 179.63s；storage 161 项、workers 52 项随后全部通过。进程检查未发现遗留 workerd/platformd 或本仓库 Gate 进程。Node JSON 测试有 `MODULE_TYPELESS_PACKAGE_JSON` 的模块类型检测提示，但三项测试均成功；未为消除提示修改父 Lynx OS 项目。

## Crash matrix 证据索引

下表标明验证落点，不将“已有测试源码”自动等同于最终 Aggregate Go。SQLite transaction 本身没有 HTTP response；事务内 abort 验证原子性，SIGKILL 验证 durable boundary，真实 HTTP 矩阵另行验证 Known/Unknown 分类。

| 设计 §18 边界 | 验证落点 |
| --- | --- |
| creating ref commit 前 | `storage/src/workflows/atomicity_tests.rs` 的 reservation rollback；`workers/src/workflow_crash_tests.rs` 的 before-reserve SIGKILL |
| creating ref 后、scheduler insert 前 | SIGKILL reserved：保持 artifact pin，grace 内不删除，超时安全释放 |
| scheduler insert 后、finalize 前 | SIGKILL inserted：重开双库、reconcile finalize、exactly one instance |
| create response 前 | `workflow_support/transport_faults.rs`：真实 facade 的 get、duplicate 与零 reservation 检查 |
| run claim transaction 中 | `storage/src/scheduler/workflow/atomicity_tests.rs`：claim abort，无半 token |
| run claim 后、dispatch 前 | SIGKILL run-claimed：lease expiry 后领取新 token |
| run 开始、首 step claim 前 | HTTP claim-before Unknown：无 step row，保留 lease 后重新 activation |
| step claim 后、callback 前 | SIGKILL step-claimed 与 HTTP claim-after Unknown：pending 恢复、新 grant |
| callback effect 后、result commit 前 | `workflow_support/product_bindings.rs`：真实 KV 重复 effect、DO 幂等 key；HTTP success-before Unknown 两次 callback report |
| result commit 后、facade response 前 | HTTP success-after Unknown：重启 isolate 后 callback report 不增加 |
| failed step 后、instance error 前 | SIGKILL step-failed 与 HTTP failure-after Unknown：failed replay、不再次调用 callback |
| last step 后、run output 前 | SIGKILL step-completed；Product Gate lost-response 与 snapshot replay |
| terminal 后、ref release 前 | SIGKILL terminal：status 保持终态，只补 release；release transaction abort 不丢 live ref |
| lease 过期后的旧 completion | storage run/step token 测试、HTTP matrix 的旧 run terminal commit 拒绝 |
| definition 切换 version | Product Gate V1/V2 frozen target；control version switch transaction rollback 测试 |
| maintenance snapshot | `workflow_support/snapshot_restore.rs`：P1 双库 manifest 校验、fresh host、fresh generation、completed replay |

以上文件路径分别相对 `crates/` 或 `crates/service/tests/`。version switch rollback、disk-pressure 与十个 SIGKILL 边界均已在最新全量测试与 P2.4 Gate 中通过；最终 P0/P1/P2/G0 aggregate 与 coverage 也已通过。

## 操作约定

控制 API 使用现有 admin authentication：

1. `POST /v1/accounts/{account}/workflows`，body `{"name":"orders"}`。
2. `POST /v1/accounts/{account}/workflows/{definition}/versions`，body `{"deploymentId":"…","className":"Orders"}`；目标必须是同 account 的 ready deployment。
3. caller deployment 声明 `{"kind":"workflow","id":"<definition UUID>"}` binding。租户仅通过 `env.NAME.create/get` 使用它。

版本探测 Unknown 保留 validating，后台 bounded reconcile 继续验证；明确无效 class 会 rejected，不替换旧 current。定义可 GET/PATCH/DELETE；存在 caller binding 或 live instance ref 时 DELETE 拒绝。GET definitions/versions/instances/steps 支持 `after` 与 `limit`（最多 1000）；版本 cursor 为 version number，steps 为 ordinal，其余为内部 UUID。

`GET /v1/operator/workflows`、`POST /v1/operator/workflows/reconcile`、`GET /v1/scheduler` 和 `platformd doctor` 只暴露有界状态与计数，不返回 payload/output/token。单次 doctor 抽样 32 个 history；完整页明确标记为 sample，不伪称穷尽验证。

`scheduler recover-corrupt` 只允许从可验证的 alarm-only control authority 重建 projection。control 中存在 Queue、Cron activation 或任何 Workflow instance referrer 时，命令在隔离文件前拒绝空库重建，要求整机 snapshot restore；released/terminal Workflow history 也不能丢弃。详见 [Scheduler 恢复](../references/runbooks/scheduler-recovery.md)。

默认 Workflow pool 启用，16 个 in-flight；lease/heartbeat/dispatch deadline 为 60s/20s/300s。单 JSON 值最多 1 MiB，单实例 1024 steps / 32 MiB；全部终态历史计入 quota，P2.4 不自动清除。

新增 Workflow metrics 后固定序列预算为 517；旧配置若显式 `metrics.max_series = 512`，必须先提高至至少 517（默认示例为 1024）。默认 Workflow policy 不改变既有签名 snapshot policy hash；非默认 Workflow policy 纳入 fingerprint。旧快照仍遵循 P1 的 exact-source-release restore、再 forward upgrade 合同。

Workflow snapshot 恢复不撤销 KV/D1/DO/R2/Queue 中已发生的外部效果。step result commit 前的失败可能重跑 callback，应使用 instance ID + step name + count 构造业务幂等键；这不是跨产品 exactly-once 事务。
