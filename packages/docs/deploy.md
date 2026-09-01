# 部署

本节只覆盖如何把**已经发行的**匹配 OS/CPU `ocd` 文件跑成长期服务：container、systemd、launchd。不讲如何从源码编出这个文件。

共同契约：一个 `ocd`、一份绝对路径配置、一块可写可执行的 data-dir、外接 S3。永远不要把凭据写进镜像、unit、plist 或发行包；用配置里的 env/file 引用。重启依据是进程退出或 `/health/live` 失败，**不要**因 `/health/ready` 的 503 重启。

示例在仓库 `examples/container/`、`examples/systemd/`、`examples/launchd/`。

## 路径约定

Linux 示例：

| 用途 | 路径 |
| --- | --- |
| 二进制 | `/opt/open-compute/ocd` |
| 配置 | `/etc/open-compute/config.toml` |
| data-dir | `/var/lib/open-compute` |

launchd 示例用 `/usr/local/etc/open-compute/config.toml` 和 `/usr/local/var/open-compute`。部分内嵌 runbook 把配置写成 `platform.toml`；`--config` 接受你给出的绝对路径。

## systemd

单元文件见 `examples/systemd/open-compute.service`。要点：

- `Type=simple`，`ExecStart=/opt/open-compute/ocd --config /etc/open-compute/config.toml run`
- `KillMode=control-group`，`KillSignal=SIGTERM`，`TimeoutStopSec=30`。ocd 拥有 workerd 子进程，必须整组杀掉。
- `Restart=on-failure`，`RestartSec=2`：只在进程死亡 / liveness 失败时重启。
- **不要** `ExecStartPost` 去 curl `/health/live`：STARTING 阶段的一次性竞态不能把健康进程判失败并重启。持续的 liveness 监控针对 `GET /health/live`；**永远不要**因 `/health/ready` 503 重启。
- `EnvironmentFile=-/etc/open-compute/environment`。凭据留在 EnvironmentFile 或配置引用的文件里。
- `NoNewPrivileges=yes`，`ProtectSystem=strict`，`ReadWritePaths=/var/lib/open-compute`，`ReadOnlyPaths=/opt/open-compute`。

## launchd

plist 见 `examples/launchd/dev.open-compute.ocd.plist`。

- `Label`：`dev.open-compute.ocd`，来自项目域名 `open-compute.dev` 的 reverse-DNS 标识
- 参数：`/opt/open-compute/ocd --config /usr/local/etc/open-compute/config.toml run`
- `WorkingDirectory`：`/usr/local/var/open-compute`
- `RunAtLoad` true；`KeepAlive` 在非 SuccessfulExit 时拉起；`ThrottleInterval` 2；`ExitTimeOut` 30
- 日志：`/usr/local/var/log/open-compute/ocd.out.log` 与 `.err.log`

同样不要把密钥写进 plist。KeepAlive 对应进程退出，不要另接 readiness 503 去 unload/load。

## 容器

镜像说明见 `examples/container/README.md`，Dockerfile 见同目录。

- 构建上下文里只有一个名为 `ocd` 的原生 Linux 发行文件，CPU 架构必须匹配。
- 基础镜像 `ubuntu:24.04`（glibc）。upstream workerd 不是静态 ELF，不能用 `scratch` 或 Alpine/musl。
- 非 root 运行（`USER 65532`）。预先准备该 UID 所有、mode 0700 的 data-dir。
- PID 1 是 `ocd`，由它拥有并回收 workerd。没有 shell 或 runtime sidecar。
- 把可写、**可执行**的文件系统挂到 `/var/lib/open-compute`。不要 `noexec`。
- 只读挂载 `/etc/open-compute/config.toml`。先用 `ocd config init --data-dir /var/lib/open-compute` 生成，再填 S3 和监听。
- 镜像根只读。把 public listener 暴露到非 loopback 时，必须有显式 admin 认证引用，或单独的 loopback-only admin listener。
- S3 凭据用环境变量或配置引用的文件；不要烤进可执行文件或镜像。
- 重启策略：进程退出或 `/health/live` 失败，不是 readiness 503。
- 构建镜像是一次部署操作，不是默认的本地校验命令。
