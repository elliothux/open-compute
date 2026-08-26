# P2.2 Queue Producer 本地验证结果

## 结论

- P2.2 Queue lifecycle、immutable producer binding、普通 Worker 的 `send()`、`sendBatch()`、
  `metrics()`、durable SQLite transaction、delay、quota、retention、restart、snapshot/restore 和
  operator surface 已实现并通过本地 Exit Gate。
- 最终结论为 **Conditional Go**：普通 Worker 与 named WorkerEntrypoint producer 可用；Durable
  Object producer稳定 fail closed，返回 `QUEUE_DO_OUTPUT_GATE_UNSUPPORTED`，且不写入消息。
- P2.2 aggregate 在三个 fresh-process round 中通过；随后递归通过 P2.1、P1、P0 和 G0 回归。
- 这是本地实现与验证结果，不是 release package、发布或部署声明。

## Release identity

- Checkout baseline：`cb27a75c459f0bb510d275d37657e0f9c96cf7bb`
- Working tree：包含本次尚未提交的 P2.2 实现；没有 commit、push、package 或 deploy
- Host：`Darwin 25.6.0 arm64`
- Stock workerd pin：`v1.20260823.1`，expected output `workerd 2026-08-23`
- Stock workerd SHA-256：
  `8c6562229cc652bcb8d926f5cd3a80e4947d723567588635fbaaebca9fdd7577`
- `runtime/workerd.lock.json` SHA-256：
  `dc57a2451d60692e9c4808c02687c4e98e5b5e83cf268e90ab64be364833c047`
- `share/default-config.toml` SHA-256：
  `c8250dca6be0bbb872e5357a05a3b553089aaa7ca72533980f19164301782834`

## 验证矩阵

| 范围 | 命令 | 结果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all --check` | PASS |
| Rust lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| Workspace tests + real-runtime Gates | `./scripts/coverage.sh` | PASS；43,688 / 48,521 lines，90.039364% |
| No-default-features | `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| Rust 1.98 MSRV | `cargo +1.98.0 check --workspace --all-targets` | PASS |
| Metadata | `cargo metadata --no-deps --format-version 1` | PASS |
| Dependency boundaries | `./scripts/check-boundaries.sh` | PASS |
| Shell syntax | `sh -n scripts/*.sh` | PASS |
| Diff whitespace | `git diff --check` | PASS |
| P2.2 final aggregate | `./scripts/test-p2-2.sh` | PASS；3 fresh-process rounds |
| Recursive regression | P2.1 / P1 / P0 / `./poc/g0 test all` | PASS |

`./scripts/test-p2-2.sh` 的最终三轮运行覆盖真实 pinned workerd Queue facade/backend、ordinary 与 named
entrypoint、cold/warm isolate、restart persistence、config/delete fence、跨 account 隔离、immutable
descriptor、commit 后 SIGKILL、snapshot/restore、production binary Gate-marker 检查，并递归执行完整
P2.1/P1/P0/G0 回归。G0 verdict 仍为 Conditional Go，唯一接受限制是生成报告中的精确
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
