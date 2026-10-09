# Stress tests with anomaly detection

Stress scripts measure latency and concurrency, but **fail closed** on technical anomalies. A run with acceptable p95 but secret leakage, unexpected restarts in the selected Compose project, or cross-binding drift is a failure.

Artifacts live under `.temp/stress-run/<run-id>/`. Failed runs copy evidence to `.temp/stress-run/<run-id>/failed/anomalies.json`.

## Prerequisites

```bash
./test/stress/deploy-stress-demo.sh
source .temp/stress-run/.deploy_env
```

`deploy-stress-demo.sh` resolves the live KV namespace and D1 database IDs from the running ocd instance (via `cf kv namespaces list` / `cf d1 list`), supplies their IDs through explicit environment values without modifying tracked configuration. On first deploy it uses a two-pass flow: bootstrap without the self-referencing `SERVICE` binding, then redeploy with `SERVICE` once `stress-demo` exists.

## Release qualification

`./test/stress/qualify-release.sh ABS_CANDIDATE ABS_PACKAGE_REPORT` verifies the packaged Linux x64 executable and its source/runtime identity, builds local images, and runs smoke, P0, scenario, full P1 peak, and a one-hour soak in a unique Compose project. Docker enforces 2 CPUs and 4 GiB memory. No host-wide workerd search or signal is permitted; injected restarts select this project's `ocd` service and count only successful restarts with verified readiness and reconciliation.

Release qualification rejects abbreviated profiles, empty samples, excessive errors or latency, missing stack results, and failed recovery. The selected candidate digest and actual container limits accompany the results. Successful fixtures are torn down; failed project volumes and diagnostics remain under `.temp/release-stress/` for investigation. Release publication requires this job to pass and includes branded `test-report.html` plus `test-report.json`, bound into the manifest and SHA256SUMS.

## Scripts

| Script                 | Purpose                                                          |
| ---------------------- | ---------------------------------------------------------------- |
| `stress-smoke.sh`      | `/stack/*` route schema, `ok:true`, secret-leak checks           |
| `stress-p0.sh`         | 2C/4G concurrency ladder + KV read-after-write + DO monotonicity |
| `stress-p1.sh`         | P1 per-stack PEAK sustained load (2C/4G calibrated SLOs)         |
| `stress-p1-soak.sh`    | 1h soak: stack rotation, container restart, reconcile recovery   |
| `stress-scenario.sh`   | Mega-checkout success or fault-mode structured errors            |
| `reconcile.sh`         | Cross-binding consistency (KV, D1, R2, DO, queue, idempotency)   |
| `lib/anomaly-check.sh` | Shared detection helpers                                         |

## Result schema (v2)

```json
{
  "stacks": {
    "kv": {
      "samples": 0,
      "latency_ms": {},
      "anomalies": [],
      "verdict": "pass|fail"
    }
  },
  "scenario": {
    "mega-checkout": { "anomalies": [], "verdict": "pass|fail" }
  },
  "global_anomalies": [],
  "verdict": "pass|fail"
}
```

## Anomaly taxonomy

| Type                     | Severity | Meaning                                        |
| ------------------------ | -------- | ---------------------------------------------- |
| `http_status`            | error    | Response status outside allowed set            |
| `json_parse`             | error    | Body is not valid JSON                         |
| `json_field`             | error    | Required JSON field missing or wrong value     |
| `secret_leak`            | critical | Token/key/password pattern in response body    |
| `health_live`            | critical | `/health/live` not 200 after run               |
| `health_ready`           | critical | `/health/ready` not 200 after run              |
| `container_restart`      | warning  | Docker container restart count increased       |
| `kv_read_after_write`    | error    | KV GET value differs from prior PUT            |
| `do_counter_regression`  | error    | Durable Object counter not monotonic           |
| `fault_missing_fields`   | error    | Fault response missing `error_code` or `stack` |
| `fault_state_corruption` | critical | Fault mode left `consistent:true` verify state |
| `reconcile_kv`           | error    | KV order key missing after checkout            |
| `reconcile_d1`           | error    | D1 committed row missing for order             |
| `reconcile_r2`           | error    | R2 receipt object missing                      |
| `reconcile_do`           | error    | DO inventory counter below minimum             |
| `reconcile_queue`        | error    | Queue label not processed by consumer          |
| `reconcile_idempotency`  | error    | Idempotency KV entry missing                   |
| `reconcile_consistency`  | error    | Verify endpoint reports `consistent:false`     |
| `reconcile_bindings`     | error    | KV/D1/R2/queue bindings misaligned             |

## Structured worker errors

stress-demo returns:

```json
{ "ok": false, "error_code": "KV_FAULT_INJECTED", "stack": "kv", "details": {} }
```

Fault injection codes: `KV_FAULT_INJECTED`, `D1_FAULT_INJECTED`, `R2_FAULT_INJECTED`, `QUEUE_FAULT_INJECTED`, `DO_FAULT_INJECTED`, `WORKFLOW_FAULT_INJECTED`, `FETCH_FAULT_INJECTED`, `OUTBOUND_FAULT_INJECTED`.

## P1 profile (PEAK sustained load)

`STRESS_PROFILE` selects load shape and SLO tables:

- `2c4g` (default): reduced peak rates for an operator-configured 2 CPU / 4 GB environment. The base Compose example does not impose these resource limits; configure them separately before comparing results with this profile.
- `8c16g`: higher concurrency and tighter SLOs for larger hosts

Per-stack PEAK phases (2C/4G defaults) with schema v2 `result.json` per run:

| Stack    | Load                          | Duration   |
| -------- | ----------------------------- | ---------- |
| http     | 25 → 50 → 75 concurrent       | 5 min each |
| kv       | 16 concurrent                 | 15 min     |
| d1       | 10 write + 20 read concurrent | 15 min     |
| r2       | 4 multipart + 10 GET/s        | 20 min     |
| queue    | 200 msg/s                     | 10 min     |
| do       | 10 RPC/s + 10 WebSocket       | 15 min     |
| workflow | 3/s                           | 15 min     |
| fetch    | 50 concurrent                 | 10 min     |
| cpu      | 8 concurrent                  | 10 min     |

The stress consumer uses batches of 100 messages. Processing markers are persisted in KV before acknowledgement; the queue profile does not serialize each message through a shared DO counter. Durable Object throughput is measured by its own profile.

Rate-based phases send one concurrent batch per second and wait for it before starting another. Actual throughput depends on request latency and client overhead; the recorded sample count is authoritative. `scenario_mega` is an optional peak selection (`STRESS_P1_STACK=scenario_mega`), with 15 concurrent requests for 15 minutes.

Inter-stack cooldown defaults to 90s (`STRESS_P1_COOLDOWN_SEC` to override). Disk preflight requires ≥5 GB free on `.temp/`.

Soak (`stress-p1-soak.sh`): rotate stacks every 5 min for 1 hour; restart container every 15 min; post-restart ready + reconcile.

Abbreviated runs (shorter session): `STRESS_P1_ABBREV=1` (~10% duration/rate), `STRESS_SOAK_ABBREV=1` (15 min soak).

SLO thresholds (2C/4G PEAK, calibrated from investigation b689226b):

| Stack         | p95    | p99     | max error rate |
| ------------- | ------ | ------- | -------------- |
| http          | 2500ms | 3500ms  | 1%             |
| kv            | 5000ms | 6000ms  | 1%             |
| d1            | 3000ms | 10000ms | 5%             |
| r2            | 1000ms | 3000ms  | 15%            |
| queue         | 900ms  | 2000ms  | 1%             |
| do            | 1500ms | 3000ms  | 15%            |
| workflow      | 2000ms | 4000ms  | 10%            |
| fetch         | 2000ms | 3000ms  | 1%             |
| mega-checkout | 3000ms | 6000ms  | 5%             |

### 2C/4G accepted limitations

On the 2 CPU / 4 GB compose profile, sustained PEAK load on R2, Durable Objects, and Workflow can exceed platform capacity. Under peak concurrency the runtime may return `RUNTIME_INTERNAL` (HTTP 500) when workerd isolate scheduling or memory pressure causes transient dispatch failures. These are **accepted** at peak provided:

- Error rate stays within the stack SLO ceiling (15% for r2/do, 10% for workflow)
- Post-run `/health/ready` succeeds and no orphan `workerd` processes remain
- Functional checks pass: KV read-after-write, DO counter monotonicity, reconcile consistency

KV PEAK concurrency for `2c4g` is capped at 16 to stay within the default `max_readers_per_namespace = 2` admission window (`share/default-config.toml`); higher client concurrency mostly queues at the platform rather than increasing useful read throughput.

D1 writes use client-side `withD1Retry` (see `examples/stress-demo/src/lib/d1-retry.ts`); transient `D1_BUSY` responses are retried and do not count as SLO failures when the retry succeeds.

Workflow checkout uses `withWorkflowRetry` (see `examples/stress-demo/src/lib/workflow-retry.ts`); exhausted retries fail closed with HTTP 503 (`WORKFLOW_RUNTIME_UNAVAILABLE`) instead of ambiguous 404/500 under peak load.

Reconcile and soak post-restart paths wait up to 60s for bindings alignment (`STRESS_BINDINGS_ALIGN_SEC`, default 40 elsewhere).

R2 GET uses `withR2Retry` in the stress-demo Worker (see `examples/stress-demo/src/lib/r2-retry.ts`) plus bounded harness retry via `stress_request_idempotent_get` for transient 5xx under peak load.

## Examples

```bash
./test/stress/stress-smoke.sh
./test/stress/stress-p0.sh
./test/stress/stress-scenario.sh
./test/stress/stress-p1.sh
./test/stress/stress-p1-soak.sh
./test/stress/stress-all.sh --p1
STRESS_P1_ABBREV=1 STRESS_PROFILE=2c4g ./test/stress/stress-p1.sh
STRESS_P1_ABBREV=1 STRESS_PROFILE=2c4g STRESS_P1_STACK=kv ./test/stress/stress-p1.sh
STRESS_SOAK_ABBREV=1 ./test/stress/stress-p1-soak.sh
STRESS_PROFILE=8c16g ./test/stress/stress-p1.sh
STRESS_SCENARIO_MODE=fault STRESS_FAULT_STACK=kv ./test/stress/stress-scenario.sh
./test/stress/reconcile.sh
./test/stress/reconcile.sh .temp/stress-run/<run-id>/responses.jsonl
```
