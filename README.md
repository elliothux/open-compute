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
  <img src="https://img.shields.io/badge/runtime-verified%20workerd%20fork-f38020" alt="verified workerd fork" />
  <img src="https://img.shields.io/badge/API%20inventory-2%2C203%20members-success" alt="2203 stable members and overloads" />
  <img src="https://img.shields.io/badge/rust-1.98-orange" alt="Rust 1.98" />
  <img src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey" alt="macOS | Linux" />
</p>

<p align="center">
  <a href="https://open-compute.dev">Website</a>
  · <a href="https://open-compute.dev/docs/">Docs</a>
  · <a href="https://open-compute.dev/docs/platform/compatibility/">Compatibility</a>
  · <a href="https://open-compute.dev/docs/project/">Architecture</a>
</p>

<p align="center">
  English · <a href="README.zh.md">简体中文</a>
</p>

---

## The Workers platform, running on your hardware

You already know how to write Cloudflare Workers. **open-compute runs the compatible Workers programming model** — module workers, familiar bindings, and Wrangler workflows — on a single machine you own.

**One binary. One data directory. One object authority.** Local filesystem is the default; S3-compatible storage is optional.

No Kubernetes. No Redis. No service mesh. No distributed control plane to babysit. No vendor lock-in.

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

open-compute _is_ that layer — and it ships as **one file**.

- **One binary, everything inside.** Runtime, control plane, scheduler, and every product binding. Copy it to a host, point it at a directory, and you are serving traffic.
- **Fast because it's workerd.** Worker code runs on a pinned, checksum-verified workerd fork. Isolates start in milliseconds — not one process or container per request.
- **Nothing else to run.** SQLite owns platform metadata and Local storage owns object bytes by default. S3-compatible storage is optional; neither mode needs a database or cache sidecar.
- **Pinned and verified.** The runtime and its assets are fixed and verified at build and startup. Production startup stays offline.
- **Yours completely.** You own the code, data, and machines. External services are optional and explicitly configured.

## Proof, not promises

Compatibility here is measured, not asserted. The same fixtures run against open-compute and real Cloudflare wherever the hosted API permits direct comparison.

|           |                                                                                                                 |
| --------- | --------------------------------------------------------------------------------------------------------------- |
| **2,203** | stable API members and overloads tracked across the Workers runtime and product bindings                        |
| **7 / 7** | core product surfaces checked against real Cloudflare: Workers, Cache, KV, D1, R2, Durable Objects, and Queues  |
| **1 : 1** | a production Next.js 16 build runs on Cloudflare and open-compute from the same project and deployment artifact |
| **90%+**  | required line coverage, with real processes, SQLite, and the pinned workerd runtime in acceptance tests         |

## Compatibility

Write standard module workers (`export default { fetch }`) with the bindings you already know. See the [compatibility guide](https://open-compute.dev/docs/platform/compatibility/) for exact behavior and single-node differences.

### Runtime & bindings

| Module                | Status             |
| --------------------- | ------------------ |
| Workers               | ██████████ 100% ✅ |
| KV                    | ██████████ 100% ✅ |
| R2                    | ██████████ 100% ✅ |
| D1                    | ██████████ 100% ✅ |
| Durable Objects       | ██████████ 100% ✅ |
| Alarms                | ██████████ 100% ✅ |
| Queues                | ██████████ 100% ✅ |
| Cron                  | ██████████ 100% ✅ |
| Workflows             | ██████████ 100% ✅ |
| Static Assets         | ██████████ 100% ✅ |
| Service Bindings      | ██████████ 100% ✅ |
| Cache                 | ██████████ 100% ✅ |
| Images                | ██████████ 100% ✅ |
| Version Metadata      | ██████████ 100% ✅ |
| WebSocket Hibernation | ██████████ 100% ✅ |
| Vectorize             | ██████████ 100% ✅ |
| Markdown Conversion   | ██████████ 100% ✅ |
| AI Search             | ██████████ 100% ✅ |
| Artifacts             | ██████████ 100% ✅ |

### Management

| Surface                      | Status                                                                             |
| ---------------------------- | ---------------------------------------------------------------------------------- |
| Cloudflare v4 API            | █████████░ 90% — local `/client/v4` works with Wrangler and the official SDK       |
| Wrangler                     | ██████████ 100% ✅ — Wrangler `4.127.1` deploys and manages the supported products |
| Dashboard                    | ████████░░ 80% — operator UI built on the same `/client/v4` API                    |
| Workers Logs / realtime tail | █████████░ 90% — logs, queries, `wrangler tail`, and live tail on one node         |

### Partial

| Module                  | Status                                                  |
| ----------------------- | ------------------------------------------------------- |
| Dynamic Workers         | ████████░░ 76% — core Worker Loader APIs are available  |
| Workers Standard limits | ██░░░░░░░░ 20% — planning                               |
| Workers AI              | ██░░░░░░░░ 20% — Markdown Conversion and AI Search only |

### Planning

Design is underway; bindings and APIs are not available to deploy yet.

| Module      | Status                     |
| ----------- | -------------------------- |
| Browser Run | ██░░░░░░░░ 20% — Planning. |
| Containers  | ██░░░░░░░░ 20% — Planning. |

### Not yet

Uploads or configuration that require these capabilities fail closed.

| Module                          | Status                   |
| ------------------------------- | ------------------------ |
| General Workers AI inference    | ░░░░░░░░░░ 0% — Not yet. |
| Hyperdrive                      | ░░░░░░░░░░ 0% — Not yet. |
| Analytics Engine                | ░░░░░░░░░░ 0% — Not yet. |
| Workers for Platforms           | ░░░░░░░░░░ 0% — Not yet. |
| Pipelines                       | ░░░░░░░░░░ 0% — Not yet. |
| Rate Limiting                   | ░░░░░░░░░░ 0% — Not yet. |
| mTLS certificates               | ░░░░░░░░░░ 0% — Not yet. |
| Tail Workers / traces / Logpush | ░░░░░░░░░░ 0% — Not yet. |

100% ✅ means the documented Worker or product API has no missing methods. Single-node differences are listed in the [compatibility guide](https://open-compute.dev/docs/platform/compatibility/). Live surface: `ocd capabilities --json`.

## Quick start

### Set up with an AI coding agent

Copy this prompt into Codex, Claude Code, or another coding agent:

```text
Read https://open-compute.dev/llms-full.txt and https://open-compute.dev/docs/get-started/. Install the current open-compute release and configure one local instance on this machine. Inspect any existing installation first, preserve its configuration and instance data, ask before using sudo or making destructive changes, then run ocd status and report the result.
```

Use [`llms-small.txt`](https://open-compute.dev/llms-small.txt) instead when the agent has a smaller context window.

### Set up manually

Install the release binary, create the recommended system configuration, and start the managed service:

```sh
curl -fsSL https://open-compute.dev/install.sh | sudo sh
sudo ocd setup --yes
ocd status
ocd dashboard
```

In a normal Worker project, keep Wrangler project-local for development and use `ocd wrangler` for a real open-compute target:

```sh
npm install --save-dev wrangler@4.127.1
npx wrangler dev
ocd wrangler deploy
```

Production remains **one release executable, one config, and one data directory**. Runtime payloads are embedded and verified; daemon startup does not download or search `PATH` for workerd.

For the complete installation path, remote targets, CI, environments, tail, and rollback, see [Get started](https://open-compute.dev/docs/get-started/) and [Develop](https://open-compute.dev/docs/develop/).

## Architecture

<p align="center">
  <img src="share/open-compute-architecture.png" alt="open-compute architecture" width="880" />
</p>

| Component                   | Role                                                                                          |
| --------------------------- | --------------------------------------------------------------------------------------------- |
| `ocd`                       | The control plane: ingress, API, scheduler, supervisor, and deployment authority              |
| `workerd`                   | Pinned, checksum-verified Worker runtime                                                      |
| SQLite                      | Local authoritative state — no external database, no eventual consistency                     |
| Local / S3 object authority | Bundles, static assets, R2 bytes, Artifacts, snapshots, backups, cache bodies, and AI sources |

Tenants get exactly what their deployment declares — and nothing else. No SQLite or Local object paths, no S3 credentials, no internal tokens, no sibling tenants. Enforced at the capability layer, not by convention.

### Built in Rust, engineered for the hot path

The host is a single async Rust process — no GC pauses, no interpreter, no sidecar hops between the socket and your Worker.

- **Async all the way down.** `tokio` multi-threaded runtime with `axum` + `hyper` serving both planes. Request bodies stream through as `bytes` without buffering whole payloads.
- **`unsafe_code = "forbid"`.** Workspace-wide — the entire platform is safe Rust. Plus `missing_docs = "deny"`, `unused_must_use = "deny"`, and Clippy `-D warnings` across all targets and features.
- **Release built for speed.** Full LTO, `codegen-units = 1`, `panic = "abort"`, symbols stripped — one dense, statically-linked artifact.
- **In-process state.** `rusqlite` embeds SQLite in `ocd`; transactions are function calls, not network round-trips. Foreign keys stay enabled and WAL remains locally owned.
- **Zero-copy where it counts.** Verified runtime payloads are content-addressed and materialized once, then reused across restarts.

### Layered crates with enforced boundaries

Dependency direction is checked in CI — architecture that can't silently rot:

```
core ── storage ── artifacts ── runtime      (siblings, lower level)
                    └── workers              (may use core/storage/artifacts, never runtime)
                          └── service        (composition root: CLI, HTTP, workerd bridge)
```

`ocd` compiles the runtime configuration, starts workerd as a supervised child, and communicates over a **loopback-only** channel. It owns readiness, graceful shutdown, restart backoff, and recovery.

Deployments are **immutable and content-addressed**. `workerLoader` keys are deployment identities, so promotion and rollback move a pointer — they never mutate what is already running.

## What it's not

Honest boundaries beat surprises in production:

- **Not Cloudflare's global edge.** One node on infrastructure you run — no Anycast, no cross-region replication, no POP fabric. That tradeoff is exactly what buys you strong local consistency.
- **Not a universal drop-in.** Compatibility is tracked surface by surface, and every deviation is documented rather than glossed over.
- **Not a multi-replica HA cluster.** One data directory, one process, one machine — by design.

## Documentation

| Goal                       | Start here                                                                                                                                         |
| -------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------- |
| Understand the design      | [Architecture and project guide](https://open-compute.dev/docs/project/)                                                                           |
| Check API support          | [Compatibility](https://open-compute.dev/docs/platform/compatibility/) · [Worker API index](https://open-compute.dev/docs/platform/reference/api/) |
| Track unsupported features | [Not available](https://open-compute.dev/docs/platform/unsupported/)                                                                               |
| Build and deploy Workers   | [Develop](https://open-compute.dev/docs/develop/)                                                                                                  |
| Download and release       | [GitHub Releases](https://github.com/elliothux/open-compute/releases) · [Project guide](https://open-compute.dev/docs/project/)                    |
| Run in production          | [Get started](https://open-compute.dev/docs/get-started/) · [Operate](https://open-compute.dev/docs/operate/)                                      |
| Operate and recover        | [Operate](https://open-compute.dev/docs/operate/) · [Incident guides](https://open-compute.dev/docs/ocd/incidents/current-release/)                |
| Contribute                 | [Project guide](https://open-compute.dev/docs/project/) · [AGENTS.md](AGENTS.md)                                                                   |

## Security

- One `ocd` per data directory — enforced by lock, not documentation.
- Internal tokens never appear in argv, environment, logs, status, or metrics.
- Tenant outbound is public-only; private, loopback, link-local, and metadata addresses are rejected at the address layer.

## Sponsors

This project is sponsored by **[Lynx AI](https://lynxai.work)**.

## License

Apache-2.0. The packaged open-compute workerd fork remains under the applicable upstream Cloudflare workerd licensing.
