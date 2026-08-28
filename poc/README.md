# G0 workerd runtime spike

Disposable black-box gate for one supervised stock `workerd` process. This
slice implements **G0.0** (pinned bootstrap), **G0.1** (static host runtime),
**G0.2** (`workerLoader` immutable deployments), **G0.3** (internal fetch
dispatch envelope), **G0.4** (binding-scoped host adapter), **G0.5**
(native dynamic Durable Object facets), **G0.6** (SQLite crash/restart
persistence), **G0.7** (deployment promotion/rollback with native facet
abort/delete), and **G0.8** (three-run regression and results report).

## Host

Darwin arm64. The harness is Node.js standard library. The official Cloudflare
`workerd` Darwin arm64 artifact is downloaded into `.temp/runtime-cache/` and
SHA-256 verified before every start.

Pinned release: `v1.20260826.1` (`workerd 2026-08-26`).

## Bootstrap

```sh
./poc/g0 test bootstrap
```

That command:

1. downloads or reuses the pinned official `workerd-darwin-arm64.gz`;
2. verifies archive and binary SHA-256 against `poc/workerd.lock`;
3. compiles `poc/workerd/config.capnp` with the pinned binary;
4. starts one workerd child with a per-run temp directory, free port, fixture
   disk, writable DO disk, and control-fd readiness;
5. runs the G0.0/G0.1 black-box assertions.

No extra packages or a pre-installed `workerd` on `PATH` are required.

## Loader / dispatch

```sh
./poc/g0 test loader
```

That command starts the same pinned workerd child and sends black-box HTTP
requests through ingress to the loader host. Fixture WorkerCode is assembled
from the local read-only fixture disk with an explicit compatibility date,
main module, modules, env identity, and `globalOutbound: null`. Loader keys are
`<accountId>/<workerId>/<deploymentId>` and are immutable: promotion and
rollback change only the active route.

G0.2 hard cases (L01-L08 and immutable routing) pass. G0.3 identity, entrypoint,
body, stream, error, and logging behaviors pass. Client disconnect does not
abort the loaded worker's `request.signal` on this pinned stock workerd, so
`D-abort` fails closed and `./poc/g0 test loader` is not green (24 passed / 1
failed / 0 not-run).

## Binding

```sh
./poc/g0 test binding
```

That command sends black-box HTTP requests through ingress, `workerLoader`, and
the JSRPC `FixtureKV` facade to a scoped fake backend. `deploy_a` is pinned to
`kv_fixture_a` and `deploy_b` to `kv_fixture_b`. Props are frozen from
deployment metadata before load; the adapter uses only its own `resourceId`.
Tenant body, headers, and extra method arguments cannot switch scope.

## Durable Object facets

```sh
./poc/g0 test durable-object
```

That command sends black-box HTTP requests through ingress to the static
DoSupervisor Durable Object. The supervisor loads `Counter` from real
`workerLoader` via `getDurableObjectClass("Counter")` and creates or retrieves
it only through native `ctx.facets.get(facetName, () => ({ class, id }))`.
Facet names are encoded from `doStorageId + className + objectId` and do not
include `deploymentId`. Counter fetch, RPC, SQL increment, transaction
rollback, and supervisor/facet storage isolation all use native facet SQLite.

## Recovery / facet lifecycle

```sh
./poc/g0 test recovery
```

That command uses the same pinned stock workerd, native `localDisk`, and native
`ctx.facets.get/abort/delete` APIs. It never inspects SQLite/WAL files.

G0.6 starts PID 1, increments Counter object-1 to 3, confirms RPC 3, sends a
real SIGKILL, starts PID 2 on the same binary/config/data dir, cold-loads A
again, confirms RPC 3, and increments to 4. Object-2 keeps its own value,
supervisor metadata and facet data both recover, a fresh data dir starts empty,
and an unwritable data dir still fails closed with no in-memory fallback.
Seeded in-flight SIGKILL cycles (`G0_RECOVERY_SEED`, default `0x47300607`)
classify each crashed increment as `applied`, `not-applied`, or
`result-unknown` from API-observable state only. They do not claim
exactly-once. A `failAfterWrite` business error on one facet does not corrupt
another.

G0.7 keeps one stable `doStorageId/class/object`. Promotion A→B calls native
`ctx.facets.abort(facetName, reason)` then `get()` with B's Counter class:
`codeVersion=B`, value 3, new JS nonce, same facet name. Rollback B→A repeats
abort/get: `codeVersion=A`, value 3, same storage identity. Only native
`ctx.facets.delete(facetName)` resets storage so the next get is 0. F10 records
the window where the execution target is B but abort has not been issued (old
class A still runs, storage 3). F11 records abort completed before the next
get, without guessing an unobserved codeVersion.

`./poc/g0 test recovery` exits 0 with 17 passed / 0 failed / 0 not-run.

## Full regression

```sh
./poc/g0 test all
```

That command is the G0.8 three-run regression. It runs bootstrap, loader,
binding, durable-object, and recovery sequentially in three fresh-process
rounds with recovery seeds `1194329607` (`0x47300607`), `1194329608`
(`0x47300608`), and `1194329609` (`0x47300609`). Replay one recovery round
with:

```sh
G0_RECOVERY_SEED=1194329607 ./poc/g0 test recovery
```

After the verdict is calculated, the runner writes `docs/implemented/g0-results.md` from
that run's validated evidence (temp sibling + rename). The aggregate JSON
includes relative `resultsFile: "docs/implemented/g0-results.md"`.

Exit status: Go and Conditional Go exit 0. No-Go, including a failure to write
the results report, exits nonzero.

Current evidence-derived verdict is Conditional Go: every hard matrix case
(L01-L08, B01-B03, D01-D09, R01) passed in all three rounds, and the only
allowlisted non-pass is loader `D-abort` (abortEvents 0 -> 0 each round). See
[G0 results](../docs/implemented/g0-results.md). Standalone `./poc/g0 test loader` still
fails closed on `D-abort` and exits 1 (24 passed / 1 failed / 0 not-run).

## Gate status

| Gate | Status |
| --- | --- |
| G0.0 pinned workerd bootstrap | PASS (`./poc/g0 test bootstrap`) |
| G0.1 static host runtime | PASS (`./poc/g0 test bootstrap`) |
| G0.2 `workerLoader` | PASS (L01-L08 and immutable routing) |
| G0.3 dispatch envelope | Conditional: hard identity gate passes; client disconnect does not abort loaded `request.signal` (`D-abort` fails; `./poc/g0 test loader` exits 1, 24 passed / 1 failed / 0 not-run) |
| G0.4 binding-scoped adapter | PASS (`./poc/g0 test binding`) |
| G0.5 native DO facets | PASS (`./poc/g0 test durable-object`) |
| G0.6 crash/restart persistence | PASS (`./poc/g0 test recovery`, 17 passed / 0 failed / 0 not-run) |
| G0.7 deployment/facet lifecycle | PASS (`./poc/g0 test recovery`, native abort preserves storage; native delete alone resets it) |
| G0.8 full regression | Conditional Go (`./poc/g0 test all`; see [results](../docs/implemented/g0-results.md)) |

## Failure artifacts

Successful runs delete only their own temp directory under `.temp/g0-run/`. Failed
runs keep diagnostics in `.temp/g0-run/failed/<run-id>/`.
