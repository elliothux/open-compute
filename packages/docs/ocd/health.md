# Health checks

The two HTTP probes have different jobs. systemd / container / orchestrator restart policy may bind to liveness, never to readiness.

## `/health/live` vs `/health/ready`

| Path | Success | Failure | Use |
| --- | --- | --- | --- |
| `GET /health/live` | `200` if the process is running | Unreachable / process dead | Liveness. Restart is allowed |
| `GET /health/ready` | `200` when admission succeeds | `503` with `{"code":"<REASON>"}` | Whether to send traffic. **Do not** restart from this |
| `GET /health/status` | JSON: `readiness`, `components`, redacted `supervisor` | `401` if admin auth is configured and Bearer does not match | Inspect components, not a probe |

`/health/live` returns OK as long as the HTTP server is up. It does not mean SQLite, S3, or workerd are ready.

`/health/ready` is aggregate admission. `code` is a stable `ReadinessReason`, for example `STARTING`, `READY`, `DRAINING`, `DATA_DIR_IN_USE`, `DISK_HARD_LIMIT`, `S3_UNAVAILABLE`, `S3_DEGRADED`, `RUNTIME_STARTING`, `RUNTIME_RESTART_BACKOFF`, `RUNTIME_INVALID`, `MASTER_KEY_MISMATCH`, `MIGRATION_FAILED`, `SCHEMA_TOO_NEW`, `CONFIG_INVALID`, `SCHEDULER_UNAVAILABLE`, `SCHEDULER_BACKLOG`, `DISK_SOFT_LIMIT`, `SNAPSHOT_STALE`. 503 means "do not send traffic now", including lawful startup, degrade, or drain. Restarting on 503 interrupts backoff, scrambles the workerd generation, and turns a brief degrade into a crash loop.

Component names on `/health/status`: `process`, `data_dir`, `control_db`, `master_key`, `s3`, `cache`, `runtime`, `scheduler`, `operations`. States: `starting` / `healthy` / `degraded` / `failed` / `draining`.

The listen address comes from `server.public_bind` (default `127.0.0.1:8787`). Optional dedicated `server.admin_bind`.

## `doctor` vs `doctor --full`

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --full --json
```

Both require an absolute `--config`. JSON has `schema_version` (1), `command` (`doctor`), `result` (`ok` / `failed`), and `checks[]` (`name`, `status`: `ok` / `warning` / `failed` / `skipped`, `code`, `message`, optional non-secret `value`). Any `failed` check exits with the doctor failure code.

| | `doctor` | `doctor --full` |
| --- | --- | --- |
| Purpose | Default read-only checks | Authorizes an S3 canary and a temporary workerd compile/start/stop |
| Initializes data-dir | No | No |
| Lock | SQLite/schema checks skip if another instance holds the lock | Must take the exclusive data-dir lock; do not run full while the service is up |
| When | Anytime for read-only inspection | **After the first successful `run` and a clean shutdown** |

`--full` skips `s3_canary`, `r2_canary`, and `runtime_cycle` if the lock is held or the data-dir is missing. Plain doctor also marks those three skipped and says full doctor is required.

`doctor` is not a health probe and not self-healing. Corrupt SQLite, a wrong master key, or a digest mismatch are stop conditions; looping doctor does not repair them.

On readiness failure, follow the [incident handbook](/ocd/incidents/) by symptom. Do not restart first and hope.
