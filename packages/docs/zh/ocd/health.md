# 健康检查

两个 HTTP 探针职责不同。systemd/容器/编排的重启策略只能绑 liveness，不能绑 readiness。

## `/health/live` 与 `/health/ready`

| 路径 | 成功 | 失败 | 用途 |
| --- | --- | --- | --- |
| `GET /health/live` | 进程存活则返回 `200` | 连不上 / 进程已死 | 存活。可以据此重启 |
| `GET /health/ready` | 准入成功 `200` | `503`，body `{"code":"<REASON>"}` | 是否接流量。**不要**当重启依据 |
| `GET /health/status` | JSON：`readiness`、`components`、脱敏 `supervisor` | 若配置了 admin auth 且未带对的 Bearer，则 `401` | 看组件状态，不是探针 |

`/health/live` 只要 HTTP 服务还在就返回 OK，不表示 SQLite、选定的 object authority 或 workerd 已就绪。

`/health/ready` 是聚合准入。`code` 是稳定的 `ReadinessReason`，例如 `STARTING`、`READY`、`DRAINING`、`DATA_DIR_IN_USE`、`DISK_HARD_LIMIT`、`OBJECT_STORAGE_UNAVAILABLE`、`OBJECT_STORAGE_DEGRADED`、`RUNTIME_STARTING`、`RUNTIME_RESTART_BACKOFF`、`RUNTIME_INVALID`、`MASTER_KEY_MISMATCH`、`MIGRATION_FAILED`、`SCHEMA_TOO_NEW`、`CONFIG_INVALID`、`SCHEDULER_UNAVAILABLE`、`SCHEDULER_BACKLOG`、`DISK_SOFT_LIMIT`、`SNAPSHOT_STALE`。503 表示「现在不要把流量打过来」，包括合法的启动中、降级或排空。对 503 重启会打断 backoff、打乱 workerd generation，并把短暂降级变成崩溃循环。

Local 与 S3 在 `/health/status` 中统一使用 `object_storage` 组件名。状态机是 `starting` / `healthy` / `degraded` / `failed` / `draining`。Local 的 free-space 阈值会在 maintenance 中重查；hard pressure 拒绝写入并让 readiness 失败，但不把 liveness 变成重启信号。

监听地址来自配置的 `server.public_bind`（默认 `127.0.0.1:8787`）。可选单独的 `server.admin_bind`。

## `doctor` 与 `doctor --full`

```sh
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --json
/opt/open-compute/ocd --config /etc/open-compute/config.toml doctor --full --json
```

两者与 `run` 共用同一个 exact-file config resolver。JSON 含 `schema_version`（1）、`command`（`doctor`）、`result`（`ok` / `failed`）和 `checks[]`（`name`、`status`：`ok` / `warning` / `failed` / `skipped`、`code`、`message`、可选非密钥 `value`）。任一项 `failed` 则命令以 doctor 失败码退出。

| | `doctor` | `doctor --full` |
| --- | --- | --- |
| 目的 | 默认只读检查 | 授权 object-storage/R2 canary 和一次临时 workerd 编译/启动/停止 |
| 是否初始化 data-dir | 否 | 否 |
| 锁 | 另一实例持有锁时，SQLite/schema 等检查会 skip | 必须拿到 data-dir 排他锁；服务运行时不要执行 full |
| 时机 | 随时只读探查 | **首次成功 `run` 且干净停机之后** |

`--full` 在锁被占或 data-dir 不存在时会 skip `object_storage_canary`、`r2_canary`、当前 backend capability check 和 `runtime_cycle`。普通 doctor 也会把 mutating check 标成 skip，并写明需要 full doctor。Backend-specific detail 分别使用 `local_root`、`local_format`、`local_free_space`、`local_fsync`，或 `s3_tls`、`s3_connectivity`、`s3_provider_capability`。Local 检查不暴露绝对 object path；S3 credential、endpoint error 和 provider body 同样不输出。

`doctor` 不是健康探针，也不是自愈。损坏的 SQLite、错误的 master key、digest 不匹配都是停止条件，不要靠反复 doctor 修复。

就绪失败时按 [故障手册](/zh/ocd/incidents/) 的症状走，不要先重启碰运气。
