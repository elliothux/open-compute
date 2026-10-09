#!/usr/bin/env bash
# Build and exercise the native Linux ARM64 release path in a local container.
set -Eeuo pipefail

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
image=open-compute-release-dry-run:linux-arm64

fail() {
  echo "release dry-run: $*" >&2
  exit 1
}

workspace_version() {
  awk '
    $0 == "[workspace.package]" { section = 1; next }
    section && /^\[/ { exit }
    section && /^version = "/ {
      value = $0
      sub(/^version = "/, "", value)
      sub(/"$/, "", value)
      print value
      found = 1
    }
    END { if (!found) exit 1 }
  ' "$root/Cargo.toml"
}

inside() {
  phase=$1
  : "${OPEN_COMPUTE_RELEASE_DRY_RUN_REVISION:?missing candidate revision}"
  : "${OPEN_COMPUTE_RELEASE_DRY_RUN_OUTPUT:?missing output directory}"

  export BUN_INSTALL_CACHE_DIR="$root/.temp/release-dry-run-cache/bun"
  export CARGO_HOME="$root/.temp/release-dry-run-cache/cargo-home"
  export CI=1
  export GIT_CONFIG_COUNT=1
  export GIT_CONFIG_KEY_0=safe.directory
  export GIT_CONFIG_VALUE_0="$root"
  export GIT_OPTIONAL_LOCKS=0
  export PLAYWRIGHT_BROWSERS_PATH=/ms-playwright
  export RUSTFLAGS='-D warnings'
  export RUSTUP_TOOLCHAIN=1.98.0

  [ "$(uname -s)" = Linux ] || fail "container must run Linux"
  [ "$(uname -m)" = aarch64 ] || fail "container must run Linux ARM64"
  [ "$(rustc --version | awk '{print $2}')" = 1.98.0 ] || fail "Rust 1.98.0 is required"
  [ "$(bun --version)" = 1.3.14 ] || fail "Bun 1.3.14 is required"
  [ "$(node --version)" = v26.8.1 ] || fail "Node.js 26.8.1 is required"
  [ "$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')" = 3.12 ] || fail "Python 3.12 is required"
  [ "$(getent passwd "$(id -u)" | cut -d: -f6)" = "$HOME" ] || fail "container user home is invalid"
  [ "$(git rev-parse --verify HEAD)" = "$OPEN_COMPUTE_RELEASE_DRY_RUN_REVISION" ] || fail "worktree revision changed"
  status=$(git status --porcelain --untracked-files=all)
  [ -z "$status" ] || fail "candidate worktree is dirty"
  git lfs fsck

  mkdir -p "$BUN_INSTALL_CACHE_DIR" "$CARGO_HOME"
  output=$OPEN_COMPUTE_RELEASE_DRY_RUN_OUTPUT
  case "$output" in
    "$root"/.temp/release-dry-run/output/*) ;;
    *) fail "output must stay under the release dry-run directory" ;;
  esac
  [ -d "$output" ] || fail "output directory is missing"
  runtime="$output/build-runtime"
  caddy_runtime="$output/build-caddy"
  if [ "$phase" = hydrate ]; then
    bun install --frozen-lockfile --ignore-scripts
    mbx fetch --locked
    bun scripts/prepare-workerd.ts --dest "$runtime" --download >/dev/null
    bun scripts/prepare-caddy.ts --dest "$caddy_runtime" --download >/dev/null
    return
  fi
  [ "$phase" = qualify ] || fail "unknown container phase: $phase"

  export CARGO_NET_OFFLINE=true
  mbx fetch --locked --offline
  bun test/conformance/check.ts --case baseline-identity
  node --test test/release-tools.test.mjs

  archive="$runtime/workerd-linux-arm64.gz"
  export OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE="$archive"
  export OPEN_COMPUTE_TEST_WORKERD="$runtime/workerd"
  export OPEN_COMPUTE_BUILD_CADDY="$caddy_runtime/caddy"

  version=$(workspace_version)
  destination="$output/ocd-v$version-linux-arm64"
  report="$output/release-report-linux-arm64.json"
  export CARGO_TARGET_DIR="$root/.temp/release-dry-run-cache/release-target"
  ./scripts/package-release.sh \
    --dest "$destination" \
    --archive "$OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE" | tee "$report"

  candidate_bytes=$(stat -c %s "$destination")
  candidate_sha256=$(sha256sum "$destination" | cut -d' ' -f1)
  [ "$(stat -c %a "$destination")" = 555 ] || fail "candidate mode is not 0555"
  REPORT="$report" \
  REVISION="$OPEN_COMPUTE_RELEASE_DRY_RUN_REVISION" \
  VERSION="$version" \
  BYTES="$candidate_bytes" \
  SHA256="$candidate_sha256" \
    bun -e '
      const report = await Bun.file(process.env.REPORT).json();
      if (report.schemaVersion !== 1 || report.target !== "linux-arm64"
          || report.version !== process.env.VERSION
          || report.revision !== process.env.REVISION
          || report.bytes !== Number(process.env.BYTES)
          || report.sha256 !== process.env.SHA256) {
        throw new Error("local release package identity mismatch");
      }
    '

  unset CARGO_TARGET_DIR
  OPEN_COMPUTE_TEST_OCD="$destination" \
  OPEN_COMPUTE_PACKAGE_GATE_USER_ROOT=1 \
    ./test/gate.py single-binary --jobs 1

  server_evidence="$output/dashboard-server"
  mkdir -p "$server_evidence"
  [ ! -e "$HOME/.open-compute" ] || fail "package Gate left user state behind"
  export OPEN_COMPUTE_ADMIN_TOKEN=dev-admin-token
  export OPEN_COMPUTE_DEPLOYER_TOKEN=dev-deployer-token
  export OPEN_COMPUTE_READ_ONLY_TOKEN=dev-read-only-token
  export OPEN_COMPUTE_DASHBOARD_E2E_BASE_URL=http://127.0.0.1:18787/operator/
  unset OPEN_COMPUTE_DASHBOARD_E2E_BROWSER_CHANNEL

  server_pid=
  cleanup() {
    if [ -n "${server_pid:-}" ]; then
      kill "$server_pid" 2>/dev/null || true
      wait "$server_pid" 2>/dev/null || true
      server_pid=
    fi
  }
  on_exit() {
    rc=$?
    cleanup
    if [ "$rc" -ne 0 ]; then
      find "$server_evidence" -maxdepth 1 -type f -print -exec tail -n 240 {} \; >&2 || true
    fi
    exit "$rc"
  }
  trap on_exit EXIT

  OPEN_COMPUTE_OCD_BIN="$destination" \
  OPEN_COMPUTE_DEV_PRODUCTION_SCOPE=1 \
  OPEN_COMPUTE_DEV_OCD_ROOT="$HOME/.open-compute" \
  OPEN_COMPUTE_DEV_STATE_DIR="$server_evidence" \
    ./scripts/dev-test.sh run >"$server_evidence/ocd.log" 2>&1 &
  server_pid=$!

  apps/dashboard/scripts/run-e2e.sh \
    dashboard.spec.ts lifecycle.spec.ts \
    --grep 'sign in survives page reload within the same tab|Worker create, detail, and deletion use the browser SDK'

  cleanup
  trap - EXIT
  printf 'release dry-run passed: %s\nreport: %s\n' "$destination" "$report"
}

if [ "${1:-}" = --inside ]; then
  [ "$#" -eq 2 ] || fail "usage: $0 --inside hydrate|qualify"
  inside "$2"
  exit
fi
[ "$#" -eq 0 ] || fail "usage: $0"

if [ "$(uname -s)" = Darwin ] && [ "${OPEN_COMPUTE_CAFFEINATED:-}" != 1 ]; then
  export OPEN_COMPUTE_CAFFEINATED=1
  exec /usr/bin/caffeinate -is "$0"
fi
unset OPEN_COMPUTE_CAFFEINATED

command -v docker >/dev/null 2>&1 || fail "Docker is required"
docker info >/dev/null 2>&1 || fail "Docker is not running"
host_uid=$(id -u)
host_gid=$(id -g)
[ "$host_uid" -ne 0 ] || fail "run as the owning non-root user, not through sudo"
status=$(git -C "$root" status --porcelain --untracked-files=all)
[ -z "$status" ] || fail "the current checkout must be clean so HEAD is the candidate"
revision=$(git -C "$root" rev-parse --verify HEAD)
git -C "$root" lfs fsck

state="$root/.temp/release-dry-run"
source="$state/source"
mkdir -p "$state/output"
if [ -e "$source/.git" ]; then
  status=$(git -C "$source" status --porcelain --untracked-files=all)
  [ -z "$status" ] || fail "the retained Docker worktree has tracked changes"
  git -C "$source" switch --detach "$revision"
elif [ -e "$source" ]; then
  fail "$source exists but is not a Git worktree"
else
  git -C "$root" worktree add --detach "$source" "$revision"
fi
git -C "$source" submodule update --init --depth 1 third_party/gitserver
git -C "$source" lfs checkout
git -C "$source" lfs fsck

run_id="$(date -u +%Y%m%dT%H%M%SZ)-${revision:0:12}"
host_output="$state/output/$run_id"
container_output="$source/.temp/release-dry-run/output/$run_id"
mkdir -p "$(dirname "$container_output")"
mkdir "$host_output"
common_git=$(git -C "$source" rev-parse --path-format=absolute --git-common-dir)

echo "release dry-run: building pinned Linux ARM64 toolchain image"
iid_file="$host_output/image-id"
docker build \
  --platform linux/arm64 \
  --tag "$image" \
  --iidfile "$iid_file" \
  "$source/test/release-dry-run"
grep -Eq '^sha256:[0-9a-f]{64}$' "$iid_file" || fail "Docker returned an invalid image ID"
image_id=$(<"$iid_file")
passwd_file="$host_output/passwd"
docker run --rm --pull=never --platform linux/arm64 "$image_id" cat /etc/passwd |
  awk -F: -v uid="$host_uid" '$3 != uid' > "$passwd_file"
printf 'release-dry-run:x:%s:%s:release dry-run:/home/pwuser:/bin/bash\n' \
  "$host_uid" "$host_gid" >> "$passwd_file"
chmod 0444 "$passwd_file"

container_args=(
  --rm
  --pull=never
  --platform linux/arm64
  --user "$host_uid:$host_gid"
  --workdir "$source"
  --read-only
  --cap-drop ALL
  --security-opt no-new-privileges
  --pids-limit 4096
  --shm-size 1g
  --mount "type=bind,src=$source,dst=$source"
  --mount "type=bind,src=$common_git,dst=$common_git,readonly"
  --mount "type=bind,src=$host_output,dst=$container_output"
  --mount "type=bind,src=$passwd_file,dst=/etc/passwd,readonly"
  --tmpfs "$common_git/lfs/tmp:rw,nosuid,nodev,size=512m,mode=1777"
  --tmpfs /tmp:rw,exec,nosuid,nodev,size=8g,mode=1777
  --tmpfs "/home/pwuser:rw,exec,nosuid,nodev,size=4g,mode=0700,uid=$host_uid,gid=$host_gid"
  --env HOME=/home/pwuser
  --env "OPEN_COMPUTE_RELEASE_DRY_RUN_OUTPUT=$container_output"
  --env "OPEN_COMPUTE_RELEASE_DRY_RUN_REVISION=$revision"
)

echo "release dry-run: hydrating locked dependencies"
docker run "${container_args[@]}" \
  --network bridge \
  "$image_id" "$source/scripts/release-dry-run.sh" --inside hydrate

echo "release dry-run: qualifying offline"
docker run "${container_args[@]}" \
  --network none \
  "$image_id" "$source/scripts/release-dry-run.sh" --inside qualify

echo "release dry-run: retained evidence at $host_output"
