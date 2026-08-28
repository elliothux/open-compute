# Container notes

The build context contains one native Linux release file named `platformd`. Use the matching
CPU architecture. The Ubuntu 24.04 base matches the CI Linux release builder; the upstream
workerd binary requires glibc, so this image cannot use `scratch` or Alpine/musl.

- Run as non-root (`USER 65532`). Pre-provision a data directory owned by that UID with mode 0700.
- PID 1 is `platformd`; it owns and drains the workerd child. There is no shell or runtime sidecar.
- Mount a writable, executable filesystem at `/var/lib/open-compute`. It stores databases, keys,
  locks, artifact caches, and the verified embedded runtime package. Do not mount it `noexec`.
- Mount the operator config read-only at `/etc/open-compute/config.toml`. Generate its initial
  content using `platformd config init --data-dir /var/lib/open-compute`, then set S3 and listeners.
- Keep the image root read-only. Expose a non-loopback public listener only with an explicit
  admin authentication reference or a separate loopback-only admin listener.
- Supply S3 credentials with environment variables or files referenced by config. Never bake
  credentials into the executable or image.
- Restart on process exit or `/health/live` failure, never on readiness 503.
- An image build is a deployment operation, not an automatic local validation command.

Upstream requirements: https://github.com/cloudflare/workerd/tree/v1.20260826.1#running-workerd
