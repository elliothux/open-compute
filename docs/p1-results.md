# P1 Platform Hardening 本地验证结果

## 结论

- P1.0 至 P1.7 的实现以及 functional/process/security aggregate 脚本通过。
- P1.8 verdict 为 **No-Go**：当前 pinned stock workerd 的 production DO facade 不提供
  `ctx.acceptWebSocket()`；basic WebSocket 仍由 P0.7 Gate 覆盖并通过。该条件性结论不阻塞 P2。
- 本结果是本地实现与回归证据，不是 release candidate 资格声明。1 小时 developer soak、24 小时
  release-candidate soak 以及 release package/container/one-click rehearsal 未执行。

## Release identity

- Checkout baseline：`19dd7227fed39a1c3871f7498977bf4416849844`
- Working tree：包含本次尚未提交的 P1 实现；没有 commit、push、package 或 deploy
- Host：`Darwin 25.6.0 arm64`
- Stock workerd pin：`v1.20260823.1`，expected output `workerd 2026-08-23`
- `runtime/workerd.lock.json` SHA-256：
  `dc57a2451d60692e9c4808c02687c4e98e5b5e83cf268e90ab64be364833c047`
- `share/default-config.toml` SHA-256：
  `caf4a3ede246662d51dcc2cda7a71d66ea62a4292ad211eae3f88012f187397c`

## 验证矩阵

| 范围 | 命令 | 结果 |
| --- | --- | --- |
| Rust format | `cargo fmt --all --check` | PASS |
| Rust lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | PASS |
| Workspace tests | `cargo test --workspace --all-targets --all-features -- --test-threads=1` | PASS |
| No-default-features | `RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features` | PASS |
| Rust 1.98 MSRV | `cargo +1.98.0 check --workspace --all-targets` | PASS |
| Metadata | `cargo metadata --no-deps --format-version 1` | PASS |
| Dependency boundaries | `./scripts/check-boundaries.sh` | PASS |
| Shell syntax | `sh -n scripts/*.sh` | PASS |
| Diff whitespace | `git diff --check` | PASS |
| Coverage and real-runtime Gates | `./scripts/coverage.sh` with `OPEN_COMPUTE_TEST_WORKERD` | PASS; 38,464 / 42,726 lines, 90.024809% |
| P1 aggregate | `./scripts/test-p1.sh` with `OPEN_COMPUTE_TEST_WORKERD` | PASS |
| Stock-workerd G0 | `./poc/g0 test all` | Conditional Go; only exact `loader:D-abort` limitation |
| Mixed load | `./scripts/load-p1.sh --profile mixed --seed 1701` | PASS |
| Local smoke soak | `./scripts/soak-p1.sh --duration 10m --seed 1701` | PASS |

`./scripts/test-p1.sh` completed P1 conformance, security/fuzz, crash/recovery, upgrade/rollback,
P1.8 conditional Gate, reliability, load and P0 aggregate regressions. The security/release fuzz runner
executed 9,739,783 cases in 10 seconds. P0 and P1 process Gates used the verified stock workerd rather than
a mock runtime.

## Capacity 与 soak 证据

Mixed load ran 3 iterations in 331 seconds with 183 request samples. The worst iteration measured p50
25.066 ms, p95 295.527 ms, p99 302.082 ms, maximum runner RSS 117,669,888 bytes and process-recovery Gate
51 seconds. Machine-readable evidence is under `target/p1-results/load/`.

The 10-minute mixed soak ran 3 iterations for 600 seconds with the fault schedule
`p0_combined_then_upgrade_then_platformd_sigkill`; verdict was `pass` and the event log remained bounded to
400 lines. Machine-readable evidence is under `target/p1-results/soak-10m/`.

## 已验证能力

- capability/release identity、稳定 error code、API conformance 与 compatibility deviation authority；
- 全局写入 admission、quota/disk reservation、offline data-dir ownership 与稳定失败语义；
- authenticated platform snapshot、retention/GC、fresh-host restore、post-restore scheduler/WAL normalization；
- forward-only migration 008、preflight/apply、snapshot rollback anchor、crash/restart recovery；
- malicious input/Worker、secret hygiene、support-bundle allowlist 与 release fuzz；
- bounded metrics、health/ready、doctor、backup attestation、runbook 和 operator scripts；
- P0.1 至 P0.8、P0 aggregate 与 G0 在最终本地工作树上的 stock-workerd 回归。

## 条件性限制与未验证项

- G0 remains `Conditional Go` only for the generated report's exact `loader:D-abort` observation.
- P1.8 remains `No-Go`; no hibernation API, compatibility flag, persisted frame/session path or replay shim
  was added. See `docs/p1-8-results.md`.
- 1 小时 developer soak 与 24 小时 release-candidate mixed soak 未执行，因此不能宣称满足 P1 总 Exit
  Gate 的长期稳定性/RC 条件。
- Release packaging may download the formally pinned archive and container/service rehearsal changes runtime
  state; neither was authorized or performed in this local implementation run.
