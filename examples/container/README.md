# Container notes

- Run as non-root (`USER 65532`).
- PID 1 is `platformd`. SIGTERM is drained by the process; the image does not wrap a shell.
- Mount a writable volume at `/var/lib/open-compute` for `control.sqlite`, keys, cache, and locks.
- Keep `/opt/open-compute` read-only.
- Supply S3 credentials with `-e S3_ACCESS_KEY_ID` / `-e S3_SECRET_ACCESS_KEY` or files referenced by config. Do not bake secrets into the image.
- Orchestrators should restart on process exit or `/health/live` failure, never on `/health/ready`.
