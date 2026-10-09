# Container deployment

Run one non-root `ocd` with its embedded runtime and a persistent named volume. The
Ubuntu 24.04 base matches the Linux release's glibc baseline. The one-shot init
container uses `instance setup` to create the database and master key, then starts
and stops the configured instance before marking the volume initialized.

## Published images

After the first container publication, use matching version tags for both images:

```bash
cd examples/container
cp .env.example .env
# Replace all three authentication tokens with distinct private values.
docker compose up -d
curl -fsS http://127.0.0.1:8787/health/live
```

The defaults are `ghcr.io/elliothux/open-compute:0.3.0` and
`ghcr.io/elliothux/open-compute-init:0.3.0`. Set `OC_IMAGE` and `OC_INIT_IMAGE`
together when selecting another release or registry. No local build is required.
Images are published only from an existing stable GitHub Release, after native
amd64 and arm64 empty-volume/restart tests pass. The workflow runs after the release workflow
succeeds or on explicit dispatch, never automatically publishes PR or main builds,
and does not maintain a mutable `latest` tag.

The default host listener is `127.0.0.1:8787`; `OC_PUBLIC_PORT` changes the host
port while the container continues listening on 8787. Use a distinct Compose
project (`docker compose -p other ...`) for another independent stack. Worker
HTTP routing uses the normal `<worker>.<account-id>.localhost` hostnames. Public
DNS/TLS requires the separate shared Gateway configuration documented in the
[operator guide](../../docs/references/single-binary.md); this example does not
implicitly configure a public domain or certificates.

## Build locally

Download the matching **published** `ocd` executable and its checksum manifest.
This example selects ARM64; use `linux-x64` and `ocd.linux-amd64` for AMD64:

```bash
mkdir -p ../../.temp/container-inputs
cd ../../.temp/container-inputs
curl -fsSLO https://github.com/elliothux/open-compute/releases/download/v0.3.0/SHA256SUMS
curl -fsSLO https://github.com/elliothux/open-compute/releases/download/v0.3.0/ocd-v0.3.0-linux-arm64
awk '$2 == "ocd-v0.3.0-linux-arm64" {print; found=1} END {exit !found}' SHA256SUMS | sha256sum --check -
cp ocd-v0.3.0-linux-arm64 ../../examples/container/ocd.linux-arm64
cd ../../examples/container
docker compose -f docker-compose.yml -f docker-compose.build.yml up -d --build
```

The Dockerfile copies only the selected architecture's executable. Runtime
preparation, checksum verification and compilation remain release/build tasks;
production startup does not download anything. See the [single-binary
contract](../../docs/references/single-binary.md).

## Existing S3 storage

Provision the bucket, credentials and external Docker network first. In `.env`,
set `OC_EXTERNAL_NETWORK`, `OC_S3_ENDPOINT`, `OC_S3_BUCKET`,
`OC_S3_ACCESS_KEY_ID`, and `OC_S3_SECRET_ACCESS_KEY`. Choose non-overlapping
`OC_S3_PREFIX` and `OC_S3_R2_PREFIX` for each instance, then run:

```bash
docker compose -f docker-compose.yml -f docker-compose.external-s3.yml up -d
```

Add `-f docker-compose.build.yml` when using local images. Both init and runtime
join the external network. Initialization performs the real instance startup,
including its S3 preflight; a failed preflight must not mark the volume ready.
This stack never creates a remote bucket for you.

## Persistence and recovery

- `ocd-data` contains the scope manifest, registered instance, SQLite databases,
  private keys, object data and extracted embedded runtime. Keep it writable and
  executable. The runtime image filesystem is read-only, with a temporary `/tmp`.
- Init runs as root only to prepare ownership; the daemon and its children run as
  UID/GID 65532. Runtime PID 1 is `ocd`, which owns child shutdown and recovery.
- Authentication tokens are copied to mode-0600 files on first initialization.
  Later `.env` edits do not rotate those files or reconfigure an existing instance.
  S3 credentials remain environment references; never put them into an image.
- An initialized volume is reused without rewriting authority. An interrupted,
  unmarked nonempty volume is preserved and rejected for operator recovery; do
  not erase it to retry initialization without inspecting the failure.
- `init-daemon.log` remains in the volume for bootstrap diagnostics. Init errors
  preserve the volume. Keep any recovered database and key together.
- Container health probes liveness. Instance readiness is a separate admission
  signal and must not be used as a restart trigger.

The repository regression command is `./test/container/smoke.sh` after building
`open-compute:local` and `open-compute-init:local`. It uses an isolated project,
checks initialization/restart identity and credentials, removes only its successful
test volume, and retains stopped containers/volume and `.temp/` logs on failure.
