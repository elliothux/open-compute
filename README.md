<p align="center">
  <a href="https://open-compute.dev">
    <img src="share/open-compute.png" alt="open-compute" width="480" />
  </a>
</p>

<p align="center">
  <strong>High-performance Cloudflare Workers–compatible infrastructure in a single binary, deployed in one step.</strong><br/>
  Millisecond cold starts. MB-scale memory. Zero extra dependencies.
</p>

<p align="center">
  <a href="https://open-compute.dev">Website</a>
  · <a href="docs/README.md">Docs</a>
  · <a href="packages/docs">Operator site</a>
  · <a href="docs/open-compute-workerd-platform.md">Architecture</a>
</p>

<p align="center">
  English · <a href="README.zh.md">简体中文</a>
</p>

---

## The Workers model, on your hardware

You already know how to write Cloudflare Workers. open-compute runs them — the same module workers, the same bindings, the same APIs — on a single machine you own.

One binary. One data directory. One S3 endpoint. That's the whole platform.

No Kubernetes. No Redis. No service mesh. No vendor.

## Highlights

- **One binary.** The executable carries the runtime, the control plane, and every product binding. Copy it to a host, point it at a data directory — done.
- **Fast because it's workerd.** Worker code runs on stock workerd, Cloudflare's open-source V8 runtime. Isolates start in milliseconds and idle in megabytes — not containers and gigabytes.
- **Zero extra dependencies.** SQLite holds the state. Any S3-compatible store holds the bytes. Nothing else to install, nothing else to keep running.
- **Self-hosted by default.** Your data never leaves your machines. Fully offline once deployed.
- **Verified compatibility.** The same test fixtures run on open-compute and on real Cloudflare. If the results differ, it isn't shipped.

## Proof, not promises

- **2,097 stable API members** across the Workers runtime and every product binding — implemented, tested, zero gaps.
- **Identical behavior on real Cloudflare.** Workers, Cache, KV, D1, R2, Durable Objects, and Queues return the same results on both platforms.
- **A real Next.js 16 app runs on both.** Same artifact, same behavior, on open-compute and Cloudflare.

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
| Operator API · SDK · Dashboard | In design |

Exact scope: [compatibility matrix](docs/references/cloudflare-compatibility.md) · `ocd capabilities --json`

## Quick start

Start the platform locally (requires Rust 1.98, Bun 1.3, Node 24, and the pinned workerd archive — see [docs](docs/references/single-binary.md)):

```sh
export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=/abs/workerd-darwin-arm64.gz
bun run build
./scripts/dev.sh
```

Deploy your first Worker:

```sh
bun run oc run --config examples/hello-worker/open-compute.json
```

That's it — type-checked, bundled, deployed, and served. In production, the same platform is a single executable with a config file and a data directory; no build tooling on the host.

## Architecture

<p align="center">
  <img src="share/open-compute-architecture.png" alt="open-compute architecture" width="880" />
</p>

| Component | Role |
| --- | --- |
| `ocd` | The entire control plane: routing, control API, scheduler, supervisor |
| `workerd` | The runtime, pinned and verified — your code runs on stock V8 |
| SQLite | Local, authoritative state — no external database |
| S3-compatible | Object storage you choose — bundles, assets, R2 bytes |

Tenants see only what their deployment declares. No SQLite paths, no S3 credentials, no internal tokens, no other tenants — ever.

## What it's not

- **Not Cloudflare's global edge.** Single node, no Anycast, no cross-region replication.
- **Not a drop-in for everything.** Compatibility is tracked surface by surface, and the gaps are documented.
- **Not a multi-replica HA cluster.** One data directory, one process, one machine.

## Documentation

| Goal | Start here |
| --- | --- |
| Understand the design | [Architecture & design](docs/open-compute-workerd-platform.md) |
| Check API support | [Compatibility matrix](docs/references/cloudflare-compatibility.md) |
| Build and deploy Workers | [Toolchain guide](packages/toolchain/README.md) |
| Run in production | [Single-binary guide](docs/references/single-binary.md) · [Container / systemd / launchd](examples/) |
| Operate and recover | [Runbooks](docs/references/README.md#运维手册) · [Operator site](packages/docs) |
| Contribute | [AGENTS.md](AGENTS.md) · [Testing policy](docs/references/testing.md) |

## Security

- One `ocd` per data directory — enforced by lock.
- Internal tokens never appear in argv, environment, logs, or metrics.
- Tenant outbound is public-only; private, loopback, and metadata addresses are rejected at the network layer.

## Sponsors

This project is sponsored by **[Lynx AI](https://lynxai.work)**.

## License

Apache-2.0. Packaged `workerd` remains under upstream Cloudflare workerd licensing.
