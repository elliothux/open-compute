# P2.2 Queue Producer 本地验证结果

## 结论

- P2.2 Queue lifecycle、immutable producer binding、普通 Worker 的 `send()`、`sendBatch()`、
  `metrics()`、durable SQLite transaction、delay、quota、retention、restart、snapshot/restore 和
  operator surface 已实现并通过本地 Exit Gate。
- 最终结论为 **Conditional Go**：普通 Worker 与 named WorkerEntrypoint producer 可用；Durable
  Object producer稳定 fail closed，返回 `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED`，且不写入消息。
- P2.2 aggregate 按用户要求在一个 fresh-process round 中通过；workspace real-runtime 回归覆盖并
  通过 P2.1、P1 和 P0，G0 保持精确 Conditional Go。
- 这是本地实现与验证结果，不是 release package、发布或部署声明。

## Release identity

- Checkout baseline：`15877c31f6480b51e0eb93687a55690364c2ceb6`
- Working tree：包含本次尚未提交的 workerd pin 升级；没有 commit、push、package 或 deploy
- Host：`Darwin 25.6.0 arm64`
- Stock workerd pin：`v1.20260826.1`，expected output `workerd 2026-08-26`
- Stock workerd SHA-256：
  `2d17da54d2671d6e9e7c776d56b934f60be8c140b9bac35ddf22f60d6cff9403`
- `runtime/workerd.lock.json` SHA-256：
  `d3614a6394cf85e24704954d9e7a9585fb38a2107e8eb73a02519e39add14d2e`
- `share/default-config.toml` SHA-256：
  `c8250dca6be0bbb872e5357a05a3b553089aaa7ca72533980f19164301782834`

## 验证矩阵

| 范围 | 命令 | 结果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all --check` | PASS |
| Rust lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| Workspace tests + real-runtime Gates | `./scripts/coverage.sh` | PASS；43,685 / 48,521 lines，90.033182% |
| No-default-features | `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| Rust 1.98 MSRV | `cargo +1.98.0 check --workspace --all-targets` | PASS |
| Metadata | `cargo metadata --no-deps --format-version 1` | PASS |
| Dependency boundaries | `./scripts/check-boundaries.sh` | PASS |
| Shell syntax | `sh -n scripts/*.sh` | PASS |
| Diff whitespace | `git diff --check` | PASS |
| P2.2 final aggregate | `OPEN_COMPUTE_GATE_ROUNDS=1 ./scripts/test-p2-2.sh` | PASS；1 fresh-process round |
| Full real-runtime regression | `cargo test --workspace --all-targets --all-features -- --test-threads=1` | PASS；P2.1 / P1 / P0 |
| Stock-workerd G0 | `./poc/g0 test all` | Conditional Go；仅精确 `loader:D-abort` |

`./scripts/test-p2-2.sh` 的最终一轮运行覆盖真实 pinned workerd Queue facade/backend、ordinary 与 named
entrypoint、cold/warm isolate、restart persistence、config/delete fence、跨 account 隔离、immutable
descriptor、commit 后 SIGKILL、snapshot/restore 和 production binary Gate-marker 检查。完整 workspace
测试另行覆盖 P2.1/P1/P0；G0 verdict 仍为 Conditional Go，唯一接受限制是生成报告中的精确
`loader:D-abort` 观察。

## 已验证能力

- control schema 009 与 scheduler schema 002 的 forward-only、checksummed migration，以及 P1 schema
  8/1 到 9/2 的 upgrade、crash resume、snapshot rollback；
- Queue create、rename、config、delete、force purge、referrer 与 idempotency restart reconciliation；
- deployment staging 中冻结 Queue binding、descriptor digest、account/lifecycle generation，并在
  RuntimeSource cold/warm/rollback路径复用同一 authority；
- JSON/text/bytes、generic iterable、精确 message/batch/delay boundary、显式零 delay precedence、V8
  fail closed；
- batch 单 transaction、commit-before-resolve、commit 后 response loss 为 result unknown且不自动 replay；
- backlog count/bytes/oldest timestamp、并发 quota、counter mismatch检查/修复、bounded retention sweep；
- scheduler Queue pool metrics、pause/resume、bounded maintenance和 Alarm/Queue fairness；
- forged frame、stale generation、config/delete fence、cross-account/binding探测全部 fail closed；
- capabilities、deviations、health、doctor、support bundle 与 metrics不暴露 message body或高基数标识。

## 条件性限制与未执行项

- stock workerd 的 service-facade transport不能继承 native Durable Object output gate，因此 P2.2 不在
  Durable Object 内开放 Queue producer；所有 DO path返回稳定 unsupported且 zero enqueue。
- P2.2 不实现 consumer、ack/retry、DLQ、Cron、V8 serialization、metadata、exactly-once或资源级 PITR。
- 未执行 release packaging、container/service rehearsal、发布、部署或长期 release-candidate soak。
