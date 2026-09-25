# Container notes

This image runs one `ocd` daemon for the explicitly registered instances. Each running instance
owns its own verified workerd child and data directory; the daemon owns the shared listener and
Gateway. The build context contains one native Linux release file named `ocd`; use the matching
CPU architecture. The Ubuntu 24.04 base matches the CI Linux release builder; workerd requires
glibc, so this image cannot use `scratch` or Alpine/musl.

- Run as non-root (`USER 65532`). Pre-provision `/var/lib/open-compute` and every external instance
  data directory for that UID with mode 0700.
- PID 1 is `ocd`; it owns and drains its children. There is no shell or runtime sidecar.
- Mount a writable, executable filesystem at `/var/lib/open-compute` (`OCD_DIR`). It holds
  `ocd.toml`, shared keys, Gateway state, locks, and the verified embedded runtime package. Do not
  mount it `noexec` or read-only.
- `ocd.toml` lists only each instance's `config` path and `autostart`. Each `compute.toml` must
  explicitly set `[data].path`; `instances/` is merely a convenient default location, never
  scanned. Data inside OCD_DIR must be strictly below `instances/`; external data paths are allowed.
- Supply credentials with environment variables or private files referenced by the appropriate
  config. Never bake credentials into the executable or image.
- Keep the image root read-only. Expose a non-loopback public listener only with an explicit
  admin authentication reference or a separate loopback-only admin listener.
- Local object bytes stay under each instance's data directory. S3 is optional; when instances
  share one bucket, configure non-overlapping system and R2 prefixes for each.
- Restart on process exit or `/health/live` failure, never on readiness 503.
- An image build is a deployment operation, not an automatic local validation command.

The image must use the formally pinned workerd embedded in the release executable. See
[`packages/runtime/workerd.lock.json`](../../packages/runtime/workerd.lock.json) for its identity.
