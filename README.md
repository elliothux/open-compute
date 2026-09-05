<p align="center">
  <a href="https://open-compute.dev">
    <img src="share/brand/open-compute.png" alt="open-compute" width="480" />
  </a>
</p>

<p align="center">
  <strong>High-performance Cloudflare Workers–compatible infrastructure in a single binary, deployed in one step.</strong><br/>
  Millisecond cold starts. MB-scale memory. Zero extra dependencies.
</p>

<p align="center">
  <a href="https://github.com/elliothux/open-compute/actions/workflows/ci.yml">
    <img src="https://github.com/elliothux/open-compute/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI" />
  </a>
  <img src="https://img.shields.io/badge/license-Apache--2.0-blue" alt="Apache-2.0" />
  <img src="https://img.shields.io/badge/runtime-stock%20workerd-f38020" alt="stock workerd" />
  <img src="https://img.shields.io/badge/API%20surface-2%2C097%20members-success" alt="2097 members" />
  <img src="https://img.shields.io/badge/rust-1.98-orange" alt="Rust 1.98" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey" alt="macOS | Linux" />
</p>

<p align="center">
  <a href="https://open-compute.dev">Website</a>
  · <a href="docs/README.md">Docs</a>
  · <a href="packages/docs">Operator site</a>
  · <a href="docs/implemented/open-compute-workerd-platform.md">Architecture</a>
</p>

<p align="center">
  English · <a href="README.zh.md">简体中文</a>
</p>

---

## The Workers platform, running on your hardware

You already know how to write Cloudflare Workers. **open-compute runs them unchanged** — the same module workers, the same bindings, the same APIs — on a single machine you own.

**One binary. One data directory. One object authority.** Local filesystem is the default; S3-compatible storage is optional.

No Kubernetes. No Redis. No service mesh. No control plane to babysit. No vendor.

```
   Everyone else                        open-compute
   ─────────────                        ────────────
   gateway + router                     ┌──────────────┐
   control plane                        │              │
   scheduler service          ═══>      │  ocd (1 bin) │
   Redis / Valkey cluster               │              │
   Postgres                             └──────────────┘
   K8s + operators                       + SQLite + Local/S3 objects
```

## Why open-compute

**workerd is a runtime, not a platform.** It executes isolated Workers brilliantly — and stops there. No multi-tenant routing, no durable state, no scheduling, no deployment lifecycle, no control API. Everyone who wants Workers on their own infrastructure has to build that layer.

open-compute *is* that layer — and it ships as **one file**.

- **One binary, everything inside.** Runtime, control plane, scheduler, and every product binding. Copy it to a host, point it at a directory, and you are serving traffic.
- **Fast because it's workerd.** Your code runs on stock workerd, Cloudflare's open-source V8 runtime. Isolates start in **milliseconds** and idle in **megabytes** — not containers, not gigabytes, not per-request process spawns.
- **Nothing else to run.** SQLite owns platform metadata and direct Local storage owns object bytes by default. You can select one S3-compatible authority instead; neither mode needs a sidecar.
- **Pinned and verified.** The current release pin uses stock workerd. Native limits and Loader development uses the [`elliothux/workerd` submodule](docs/workerd/README.md) at `third_party/workerd`; adopting a fork binary requires a coordinated pin update and validation.
- **Yours completely.** Your code, your data, your machines, fully offline. No account, no egress, no telemetry, no bill.

## Proof, not promises

Compatibility here is measured, not asserted. The same fixtures run against open-compute **and** real Cloudflare — and if the results differ, it does not ship.

| | |
| --- | --- |
| **2,097** | stable API members implemented across the Workers runtime and every product binding — **zero gaps** |
| **7 / 7** | product surfaces verified byte-for-byte against real Cloudflare: Workers, Cache, KV, D1, R2, Durable Objects, Queues |
| **1 : 1** | a production Next.js 16 build runs identically on Cloudflare and on open-compute — same artifact, same behavior |
| **90%+** | enforced line coverage, with real processes, real SQLite, and real workerd in every gate |

## Compatibility

Write standard module workers (`export default { fetch }`) with the bindings you already know:

| Module | Progress |
| --- | --- |
| Workers | █████████░ 95% |
| KV | █████████░ 95% |
| R2 | █████████░ 95% |
| D1 | █████████░ 95% |
| Durable Objects | █████████░ 95% |
| Queues | █████████░ 95% |
| Cron | █████████░ 95% |
| Workflows | █████████░ 95% |
| Static Assets | █████████░ 95% |
| Service Bindings | █████████░ 95% |
| Cache | █████████░ 95% |
| Images | █████████░ 95% |
| Version Metadata | ██████████ 100% |
| WebSocket Hibernation | ██████████ 100% |
| Cloudflare v4 · Wrangler · Dashboard | Core implemented; hosted qualification tracked separately |

The remaining 5% is single-node reality — global edge topology and hosted fleet quotas — not missing methods. Exact scope: [compatibility matrix](docs/references/cloudflare-compatibility.md) · `ocd capabilities --json`

## Quick start

Bring up the platform locally (needs Rust 1.98, Bun 1.3, Node 24, and the pinned workerd archive — see [docs](docs/references/single-binary.md)):

```sh
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/workerd-darwin-arm64.gz
bun run build
./scripts/dev.sh
```

Ship your first Worker:

```sh
CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4 \\
CLOUDFLARE_API_TOKEN="$OPEN_COMPUTE_DEPLOY_TOKEN" \\
CLOUDFLARE_ACCOUNT_ID="$OPEN_COMPUTE_ACCOUNT_ID" \\
bun run oc run --config examples/hello-worker/wrangler.jsonc
```

Type-checked, bundled, deployed, and served — one command. In production it is even smaller: **one executable, one config file, one data directory.** No build tooling on the host, no runtime downloads, no network required at startup.

## Architecture

<p align="center">
  <img src="share/open-compute-architecture.png" alt="open-compute architecture" width="880" />
</p>

| Component | Role |
| --- | --- |
| `ocd` | The whole control plane: ingress, control API, scheduler, supervisor, deployment authority |
| `workerd` | The runtime — pinned, checksum-verified, unmodified upstream |
| SQLite | Local, authoritative state — no external database, no eventual consistency |
| Local / S3 object authority | Bundles, static assets, R2 bytes, snapshots, backups, cache bodies, and AI Search sources |

Tenants get exactly what their deployment declares — and nothing else. No SQLite or Local object paths, no S3 credentials, no internal tokens, no sibling tenants. Enforced at the capability layer, not by convention.

### Built in Rust, engineered for the hot path

The host is a single async Rust process — no GC pauses, no interpreter, no sidecar hops between the socket and your Worker.

- **Async all the way down.** `tokio` multi-threaded runtime with `axum` + `hyper` serving both planes. Request bodies stream through as `bytes` without buffering whole payloads.
- **`unsafe_code = "forbid"`.** Workspace-wide — the entire platform is safe Rust. Plus `missing_docs = "deny"`, `unused_must_use = "deny"`, and Clippy `-D warnings` across all targets and features.
- **Release built for speed.** Full LTO, `codegen-units = 1`, `panic = "abort"`, symbols stripped — one dense, statically-linked artifact.
- **In-process state.** `rusqlite` with SQLite bundled in — transactions are function calls, not network round-trips. Foreign keys on, synchronous callbacks, WAL.
- **Zero-copy where it counts.** Verified runtime payloads are content-addressed and materialized once, then reused across restarts.

### Layered crates with enforced boundaries

Dependency direction is checked in CI — architecture that can't silently rot:

```
core ── storage ── artifacts ── runtime      (siblings, lower level)
                    └── workers              (may use core/storage/artifacts, never runtime)
                          └── service        (composition root: CLI, HTTP, workerd bridge)
```

`ocd` compiles a Cap'n Proto config with the verified binary, spawns workerd as a supervised child, and speaks to it over a **loopback-only** channel with per-generation tokens that never touch argv, env, or logs. It owns the full child lifecycle: readiness probes, process groups, bounded output capture, graceful and forced stop, reaping, restart backoff, and secret-free orphan recovery.

Deployments are **immutable and content-addressed**. `workerLoader` keys are deployment identities, so promotion and rollback move a pointer — they never mutate what is already running.

## What it's not

Honest boundaries beat surprises in production:

- **Not Cloudflare's global edge.** One node on infrastructure you run — no Anycast, no cross-region replication, no POP fabric. That tradeoff is exactly what buys you strong local consistency.
- **Not a universal drop-in.** Compatibility is tracked surface by surface, and every deviation is documented rather than glossed over.
- **Not a multi-replica HA cluster.** One data directory, one process, one machine — by design.

## Documentation

| Goal | Start here |
| --- | --- |
| Understand the design | [Architecture & design](docs/implemented/open-compute-workerd-platform.md) |
| Check API support | [Compatibility matrix](docs/references/cloudflare-compatibility.md) |
| Track remaining qualification | [Acceptance plans](docs/acceptance/README.md) |
| Build and deploy Workers | [Toolchain guide](packages/toolchain/README.md) |
| Download and release | [GitHub Releases](https://github.com/elliothux/open-compute/releases) · [Release process](docs/references/releasing.md) |
| Run in production | [Single-binary guide](docs/references/single-binary.md) · [Container / systemd / launchd](examples/) |
| Operate and recover | [Runbooks](docs/references/README.md#运维手册) · [Operator site](packages/docs) |
| Contribute | [AGENTS.md](AGENTS.md) · [Testing policy](docs/references/testing.md) |

## Security

- One `ocd` per data directory — enforced by lock, not documentation.
- Internal tokens never appear in argv, environment, logs, status, or metrics.
- Tenant outbound is public-only; private, loopback, link-local, and metadata addresses are rejected at the address layer.

## Sponsors

This project is sponsored by **[Lynx AI](https://lynxai.work)**.

## License

Apache-2.0. Packaged `workerd` remains under upstream Cloudflare workerd licensing.
