# Deploy

This page covers running an **already issued** OS/CPU-matching `ocd` file as a long-running service: container, systemd, and launchd. It is not Worker code deploy (`oc run` / `oc deploy`; see [Get started](/en/get-started)). It does not cover building that file from source.

Shared contract: one `ocd`, one absolute-path config, one writable executable data-dir, and external S3. Never embed credentials in images, units, plists, or release archives; use env/file refs from config. Restart on process exit or `/health/live` failure, **never** on `/health/ready` 503.

Examples live in `examples/container/`, `examples/systemd/`, and `examples/launchd/`.

## Path convention

Linux examples:

| Role | Path |
| --- | --- |
| Binary | `/opt/open-compute/ocd` |
| Config | `/etc/open-compute/config.toml` |
| Data-dir | `/var/lib/open-compute` |

The launchd example uses `/usr/local/etc/open-compute/config.toml` and `/usr/local/var/open-compute`. Some embedded runbooks write `platform.toml`; `--config` takes the absolute path you give it.

## systemd

Unit file: `examples/systemd/open-compute.service`.

- `Type=simple`, `ExecStart=/opt/open-compute/ocd --config /etc/open-compute/config.toml run`
- `KillMode=control-group`, `KillSignal=SIGTERM`, `TimeoutStopSec=30`. ocd owns the workerd child; kill the whole group.
- `Restart=on-failure`, `RestartSec=2`: restart on process death / liveness failure only.
- **Do not** `ExecStartPost` curl `/health/live`: a one-shot race during STARTING must not fail or restart a healthy process. Ongoing liveness monitors `GET /health/live`; **never** restart on `/health/ready` 503.
- `EnvironmentFile=-/etc/open-compute/environment`. Credentials stay in the EnvironmentFile or in files referenced by config.
- `NoNewPrivileges=yes`, `ProtectSystem=strict`, `ReadWritePaths=/var/lib/open-compute`, `ReadOnlyPaths=/opt/open-compute`.

## launchd

Plist: `examples/launchd/dev.open-compute.ocd.plist`.

- `Label`: `dev.open-compute.ocd`, the reverse-DNS identifier for the `open-compute.dev` project domain
- Arguments: `/opt/open-compute/ocd --config /usr/local/etc/open-compute/config.toml run`
- `WorkingDirectory`: `/usr/local/var/open-compute`
- `RunAtLoad` true; `KeepAlive` when the exit is not SuccessfulExit; `ThrottleInterval` 2; `ExitTimeOut` 30
- Logs: `/usr/local/var/log/open-compute/ocd.out.log` and `.err.log`

Do not put secrets in the plist. KeepAlive follows process exit; do not unload/load on readiness 503.

## Container

Notes: `examples/container/README.md`. Dockerfile in the same directory.

- The build context contains exactly one native Linux release file named `ocd`; CPU architecture must match.
- Base image `ubuntu:24.04` (glibc). Upstream workerd is not a static ELF; `scratch` and Alpine/musl are not usable.
- Run as non-root (`USER 65532`). Pre-provision a data directory owned by that UID with mode 0700.
- PID 1 is `ocd`; it owns and drains the workerd child. There is no shell or runtime sidecar.
- Mount a writable, **executable** filesystem at `/var/lib/open-compute`. Do not mount it `noexec`.
- Mount operator config read-only at `/etc/open-compute/config.toml`. Generate it with `ocd config init --data-dir /var/lib/open-compute`, then set S3 and listeners.
- Keep the image root read-only. Expose a non-loopback public listener only with an explicit admin authentication reference or a separate loopback-only admin listener.
- Supply S3 credentials with environment variables or files referenced by config. Never bake credentials into the executable or image.
- Restart on process exit or `/health/live` failure, never on readiness 503.
- Building an image is a deployment operation, not a default local validation command.
