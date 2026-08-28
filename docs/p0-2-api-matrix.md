# P0.2 Worker API compatibility matrix

This matrix is scoped to the pinned `workerd v1.20260826.1` release and the
platform policy in `runtime/config.capnp`. It is not a claim of full Cloudflare
Workers compatibility. `test/test-p0-2.sh` executes the cited real-workerd
conformance gate in three fresh test processes.

| Surface | Status | Real-test evidence / boundary |
| --- | --- | --- |
| `fetch`, `Request`, `Response`, `Headers` | supported | `p0_2_real_worker_create_validate_dispatch_promote_rollback_restart` constructs and executes each API in the tenant isolate. |
| `URL` | supported | The conformance endpoint parses an HTTPS URL and asserts its pathname. |
| `ReadableStream` request and response bodies | supported | The gate echoes a 4 MiB chunked body, verifies an early response cancels the unfinished upload, and verifies a runtime crash truncates a started response without replay. |
| Web Crypto digest | supported | The gate executes `crypto.subtle.digest("SHA-256", ...)` and checks the 32-byte result. |
| Timers | supported | The gate awaits a real `setTimeout()` callback inside the tenant isolate. |
| Default and named module entrypoints | supported | Default and `WorkerEntrypoint` dispatch pass; unknown exports return `ENTRYPOINT_NOT_FOUND`. Route creation and promotion probe named exports without invoking tenant `fetch()`. |
| `nodejs_compat` | supported when explicitly requested | A deployment with only the `nodejs_compat` flag imports `node:buffer` and executes `Buffer` in real workerd. No Node flag is injected by default. |
| WebSocket client API | supported with documented deviation | The gate proves the constructor is present. Connections remain subject to the same public-only network policy; WebSocket server/listener management is not provided by P0.2. |
| Outbound HTTP(S) `fetch()` | supported with documented deviation | Tenant global outbound is a fetch-only gateway backed by `Network(allow=["public"])`. The ordinary real gate rejects private/local/metadata and alternate-address forms. `test/test-p0-2-egress-linux.sh` additionally provides controlled public IPv4/IPv6/DNS success plus DNS-to-private and redirect-to-private rejection; CI runs it on Linux, while a local macOS run does not claim that Linux-only evidence. |
| Client-disconnect abort signal | supported with documented deviation | Platformd drops the proxy stream and does not replay. P0.2 does not promise that tenant `request.signal.aborted` becomes true, matching the accepted G0 `D-abort` limitation. |
| Service Worker syntax | unsupported | `WorkerBundleV1` requires an ES-module main module and rejects other main-module types with `BUNDLE_INVALID`. |
| Raw TCP `connect()` and Unix sockets | unsupported | The scoped outbound gateway exposes only `fetch()`; non-HTTP schemes fail deterministically and the underlying network policy denies local/Unix targets. |
| Product bindings (KV, R2, D1, DO, Queue, Workflow) | unsupported in P0.2 | Tenant `env` contains only declared vars and secrets. The gate enumerates `Object.keys(env)` and proves no platform fetcher, RuntimeSource, S3, SQLite, or product binding leaks into the isolate. |

The authoritative executable evidence is
`crates/service/tests/p0_2_runtime_gate.rs`; structural rejection and storage
evidence additionally live in the `open-compute-workers`,
`open-compute-storage`, and `open-compute-artifacts` unit suites.
