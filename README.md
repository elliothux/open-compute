# open-compute

本地 Cloudflare Workers 兼容平台（`platformd` + pinned `workerd`）。

P0.1 提供单进程宿主基础，P0.2 提供无产品 binding 的 module Worker
create/deploy/validate/promote/rollback/route/fetch、immutable vars/secrets、public-only egress、
retention/delete 与 artifact GC。一个 `platformd` 进程拥有配置、数据、S3 artifact、HTTP
control/data plane 和 pinned upstream `workerd` child；启动过程永不下载 runtime。

## Workspace

| Path | Role |
| --- | --- |
| `crates/core` | Config, errors, IDs, secrets, clock |
| `crates/storage` | Data dir lock, SQLite, identity, master key |
| `crates/artifacts` | SigV4 S3 store + verified cache |
| `crates/runtime` | workerd lock/verify/compile/supervisor |
| `crates/workers` | WorkerBundle、deployment pipeline、RuntimeSource、dispatch pins |
| `crates/service` | `platformd` CLI、health、control/data plane、workerd bridge |
| `runtime/` | Formal `workerd.lock.json`, Cap'n Proto, system workers |
| `poc/` | G0 evidence only; not the production implementation |
| `examples/` | Container, systemd, launchd |
| `scripts/` | Thin operator/CI launchers |

## Prerequisites

- Rust 1.98.0 (workspace toolchain and MSRV)
- macOS or Linux
- Official pinned `workerd` matching `runtime/workerd.lock.json` (G0 pin `v1.20260823.1`)
- `rclone` with `serve s3` support for local development
- Local S3-compatible endpoint for real runs (the Gate hosts its own fake S3; protocol is still AWS SDK SigV4)

## Config

`--config` must be an absolute file. Secrets only via `env:` / `file:` / documented `OPEN_COMPUTE_*` and S3 env/file refs. See `share/default-config.toml`.

```sh
platformd --config /abs/config.toml config check
platformd --config /abs/config.toml doctor
platformd --config /abs/config.toml doctor --full
platformd --config /abs/config.toml run
```

## Dev / test

本地开发直接运行：

```sh
./scripts/dev.sh
```

该脚本启动仅监听 `127.0.0.1:9000` 的 `rclone serve s3`，然后以前台进程启动
`platformd`。所有可持久化开发状态都留在仓库的 ignored `.data/` 中：

```text
.data/
├── s3/                  # rclone S3 root；open-compute/ 是 bucket
├── platform/            # platform data_dir，同时作为 TMPDIR
├── dev-config.toml      # 生成的绝对路径配置
└── rclone-s3.log
```

停止 `platformd` 时脚本同时停止 rclone，但保留上述数据供下次启动复用。默认使用正式 lock
匹配的 `poc/.runtime-cache/v1.20260823.1/workerd`；也可通过绝对路径
`OPEN_COMPUTE_DEV_WORKERD` 指定已有的 verified binary。首次 `./scripts/dev.sh` 完成数据目录初始化后，
开发环境检查可运行：

```sh
./scripts/dev.sh config check
./scripts/dev.sh doctor --full
```

测试与 Gate 仍使用测试进程内的 SigV4 S3 provider：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features -- --test-threads=1
./scripts/coverage.sh
RUSTFLAGS='-D warnings' cargo check --workspace --no-default-features
cargo metadata --no-deps --format-version 1
./scripts/check-boundaries.sh
./poc/g0 test bootstrap
OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260823.1/workerd" ./scripts/test-p0-1.sh
OPEN_COMPUTE_TEST_WORKERD="$PWD/poc/.runtime-cache/v1.20260823.1/workerd" ./scripts/test-p0-2.sh
```

Rust 覆盖率使用
[`cargo-llvm-cov`](https://github.com/taiki-e/cargo-llvm-cov)；macOS 可运行
`brew install cargo-llvm-cov`，其他环境可运行
`cargo install cargo-llvm-cov --locked`。`./scripts/coverage.sh` 使用
`--all-targets --all-features --test-threads=1` 口径，要求提供与正式 lock 匹配的 pinned
`workerd`，并执行真实 P0.1/P0.2 Rust Gate 路径。脚本在 `target/llvm-cov/` 生成终端摘要、
HTML、LCOV 和 JSON summary；workspace 生产 Rust 行覆盖率低于 90.00% 时失败。独立的
`tests/**`、`src/tests.rs`、`src/**/*_tests.rs`、`src/mock_s3.rs` 和测试 supervisor fixture
不计入分母，生产代码不得放入这些排除路径。覆盖率运行不执行
`poc/g0` JavaScript 黑盒测试，也不替代下方三个 fresh-process rounds 的 P0 验收。

两个 Gate 在 pinned binary 缺失或 hash 与 lock 不一致时都会直接失败，不会 skip。
`scripts/test-p0-2.sh` 在三个 fresh test process 中运行真实 `workerd`、SQLite 和 SigV4 S3 test
provider。支持面与明确偏差见 [`docs/p0-2-api-matrix.md`](docs/p0-2-api-matrix.md)。
Linux release CI 另以 `scripts/test-p0-2-egress-linux.sh` 创建短期受控 dual-stack 网络夹具，补齐
public IPv4/IPv6/DNS allow 与 redirect/DNS-to-private deny；该脚本会要求显式 sudo 授权并在退出时
清理地址和 hosts 项。

## Release layout (offline start)

```
open-compute/
├── bin/platformd
├── bin/workerd
├── runtime/workerd.lock.json
├── runtime/config.capnp
├── runtime/system-workers/
├── licenses/
└── share/default-config.toml
```

Package (fetch/verify official archive **only** at packaging time; refuses checksum/version mismatch and overwrite):

```sh
./scripts/package-release.sh --dest /abs/open-compute-release --download
```

## Operations

- Container: `examples/container/` — non-root, `platformd` as PID 1, writable data volume, read-only runtime.
- systemd: `examples/systemd/open-compute.service` — `KillMode=control-group`; restart on process/liveness failure, not on readiness 503.
- launchd: `examples/launchd/io.opencompute.platformd.plist`.

Never embed credentials in units/images; use env or file refs from config.

## Security / platform boundaries

- One `platformd` per data dir (`DATA_DIR_IN_USE`).
- Internal workerd token is not on argv/env/logs/status/metrics.
- `/health/live` is liveness only; `/health/ready` is admission and must not restart the process.
- P0.2 不向 tenant 暴露 KV/D1/R2/DO/Queue 等产品 binding，也不提供 multi-node HA。
- tenant outbound 仅为 HTTP(S) fetch，并由 pinned workerd 的 `Network(allow=["public"])`
  拒绝 private/local/metadata 网络目标。
- Next-start orphan recovery uses a secret-free child lease (PID + start identity + binary digest) and will not signal a reused PID.

## License

Apache-2.0. Packaged `workerd` remains upstream Cloudflare workerd.
