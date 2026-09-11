#!/bin/sh
# Install a formal open-compute `ocd` release into PATH.
#
# Authority: GitHub Releases for elliothux/open-compute (immutable assets).
# open-compute.dev may link humans to the same assets; this script downloads
# release.json, SHA256SUMS, and the target binary directly from GitHub Releases
# (or an injectable mirror base). It never creates config, data dirs, tokens,
# or OS services — use `ocd setup` after install.
#
# Usage (system-wide under /usr/local; review before elevating):
#   curl -fsSL -o install.sh https://raw.githubusercontent.com/elliothux/open-compute/main/scripts/install.sh
#   less install.sh
#   sudo sh install.sh
#
# Per-user alternative:
#   OPEN_COMPUTE_INSTALL_PREFIX="$HOME/.local" sh install.sh
#
# Environment (optional):
#   OPEN_COMPUTE_RELEASE_TAG          exact tag (vX.Y.Z); default = latest stable
#   OPEN_COMPUTE_INSTALL_PREFIX       default /usr/local  → $PREFIX/bin/ocd
#   OPEN_COMPUTE_RELEASE_DOWNLOAD_BASE  default
#     https://github.com/elliothux/open-compute/releases/download
#   OPEN_COMPUTE_GITHUB_API_BASE      default https://api.github.com
#   OPEN_COMPUTE_INSTALL_DEST         override exact binary destination path
#   OPEN_COMPUTE_RECEIPT_PATH         override install receipt path
#
# Dry review: set the injectable bases to a local file:// or https fixture
# directory that serves the same asset names as a GitHub Release tag folder.
set -eu

REPO="elliothux/open-compute"
DOWNLOAD_BASE="${OPEN_COMPUTE_RELEASE_DOWNLOAD_BASE:-https://github.com/${REPO}/releases/download}"
API_BASE="${OPEN_COMPUTE_GITHUB_API_BASE:-https://api.github.com}"
PREFIX="${OPEN_COMPUTE_INSTALL_PREFIX:-/usr/local}"
DEST="${OPEN_COMPUTE_INSTALL_DEST:-${PREFIX}/bin/ocd}"
RECEIPT="${OPEN_COMPUTE_RECEIPT_PATH:-${PREFIX}/share/open-compute/install-receipt.json}"

umask 022

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

path_permission_error() {
  label=$1
  directory=$2
  printf 'install.sh: cannot write %s directory: %s\n' "${label}" "${directory}" >&2
  printf 'install.sh: system-wide install: sudo sh install.sh\n' >&2
  printf 'install.sh: per-user install: OPEN_COMPUTE_INSTALL_PREFIX="$HOME/.local" sh install.sh\n' >&2
  exit 1
}

preflight_writable_directory() {
  label=$1
  directory=$2
  mkdir -p "${directory}" 2>/dev/null || path_permission_error "${label}" "${directory}"
  [ -d "${directory}" ] || die "${label} directory is not a directory: ${directory}"
  probe=$(mktemp "${directory}/.open-compute-install-write.XXXXXX" 2>/dev/null) \
    || path_permission_error "${label}" "${directory}"
  rm -f "${probe}" || die "failed to remove install preflight file from ${directory}"
}

preflight_install_paths() {
  bin_dir=$(dirname "${DEST}")
  receipt_dir=$(dirname "${RECEIPT}")
  preflight_writable_directory "binary" "${bin_dir}"
  if [ "${receipt_dir}" != "${bin_dir}" ]; then
    preflight_writable_directory "receipt" "${receipt_dir}"
  fi
}

detect_target() {
  os=$(uname -s | tr '[:upper:]' '[:lower:]')
  arch=$(uname -m)
  case "${os}" in
    darwin)
      case "${arch}" in
        arm64 | aarch64) printf 'darwin-arm64\n' ;;
        *) die "unsupported macOS CPU '${arch}' (official releases: darwin-arm64 only)" ;;
      esac
      ;;
    linux)
      case "${arch}" in
        x86_64 | amd64) printf 'linux-x64\n' ;;
        aarch64 | arm64) printf 'linux-arm64\n' ;;
        *) die "unsupported Linux CPU '${arch}' (official releases: linux-x64, linux-arm64)" ;;
      esac
      ;;
    *)
      die "unsupported OS '${os}' (official releases: macOS arm64, Linux x64/arm64)"
      ;;
  esac
}

resolve_tag() {
  if [ -n "${OPEN_COMPUTE_RELEASE_TAG:-}" ]; then
    printf '%s\n' "${OPEN_COMPUTE_RELEASE_TAG}"
    return 0
  fi
  # Latest stable: GitHub Releases API; reject prerelease drafts via API fields.
  need_cmd curl
  body=$(curl -fsSL --max-time 30 \
    -H "Accept: application/vnd.github+json" \
    -H "User-Agent: open-compute-install" \
    "${API_BASE}/repos/${REPO}/releases/latest") || die "failed to resolve latest release from GitHub API"
  tag=$(printf '%s' "${body}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  [ -n "${tag}" ] || die "GitHub latest release response missing tag_name"
  printf '%s\n' "${tag}"
}

is_stable_tag() {
  printf '%s' "$1" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    die "need sha256sum or shasum"
  fi
}

lookup_sum() {
  # SHA256SUMS lines: "<hex>  <filename>"
  sums_file=$1
  name=$2
  awk -v name="${name}" '$2 == name { print $1; found=1 } END { exit !found }' "${sums_file}"
}

refuse_foreign_destination() {
  if [ ! -e "${DEST}" ] && [ ! -L "${DEST}" ]; then
    return 0
  fi
  if [ -L "${DEST}" ]; then
    die "refusing to overwrite symlink at ${DEST}"
  fi
  if [ -f "${RECEIPT}" ]; then
    method=$(sed -n 's/.*"method"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${RECEIPT}" | head -n 1)
    case "${method}" in
      install.sh | manual) ;;
      *)
        die "refusing to overwrite package-manager-owned install (receipt method=${method:-unknown})"
        ;;
    esac
    receipt_bin=$(sed -n 's/.*"binary_path"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${RECEIPT}" | head -n 1)
    if [ -n "${receipt_bin}" ] && [ "${receipt_bin}" != "${DEST}" ]; then
      die "existing receipt owns a different binary path (${receipt_bin})"
    fi
    return 0
  fi
  case "${DEST}" in
    */Cellar/* | */opt/homebrew/* | */linuxbrew/* | /usr/bin/ocd)
      die "refusing to overwrite package-manager path ${DEST} without an open-compute install receipt"
      ;;
  esac
  die "refusing to overwrite existing ${DEST} without a matching install receipt; remove it or set OPEN_COMPUTE_INSTALL_DEST"
}

main() {
  need_cmd curl
  need_cmd mktemp
  need_cmd mkdir
  need_cmd mv
  need_cmd chmod
  need_cmd awk
  need_cmd sed
  need_cmd grep
  need_cmd head
  need_cmd dirname
  need_cmd rm
  need_cmd uname
  need_cmd tr

  target=$(detect_target)
  refuse_foreign_destination
  preflight_install_paths
  tag=$(resolve_tag)
  is_stable_tag "${tag}" || die "release tag must be stable SemVer vX.Y.Z (got ${tag})"
  version=${tag#v}
  asset="ocd-${tag}-${target}"
  base="${DOWNLOAD_BASE}/${tag}"

  work=$(mktemp -d "${TMPDIR:-/tmp}/open-compute-install.XXXXXX")
  trap 'rm -rf "${work}"' EXIT INT TERM

  printf 'install.sh: fetching %s (%s)\n' "${tag}" "${target}" >&2
  curl -fsSL --max-time 60 -o "${work}/release.json" "${base}/release.json" \
    || die "failed to download release.json"
  curl -fsSL --max-time 60 -o "${work}/SHA256SUMS" "${base}/SHA256SUMS" \
    || die "failed to download SHA256SUMS"
  curl -fsSL --max-time 600 -o "${work}/${asset}" "${base}/${asset}" \
    || die "failed to download ${asset}"

  expected_manifest=$(lookup_sum "${work}/SHA256SUMS" "release.json") \
    || die "SHA256SUMS missing release.json"
  actual_manifest=$(sha256_file "${work}/release.json")
  [ "${expected_manifest}" = "${actual_manifest}" ] || die "release.json checksum mismatch"

  expected_bin=$(lookup_sum "${work}/SHA256SUMS" "${asset}") \
    || die "SHA256SUMS missing ${asset}"
  actual_bin=$(sha256_file "${work}/${asset}")
  [ "${expected_bin}" = "${actual_bin}" ] || die "binary checksum mismatch"

  # release.json identity checks (fail closed on mismatch).
  manifest_version=$(sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${work}/release.json" | head -n 1)
  manifest_tag=$(sed -n 's/.*"tag"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' "${work}/release.json" | head -n 1)
  [ "${manifest_version}" = "${version}" ] || die "release.json version mismatch"
  [ "${manifest_tag}" = "${tag}" ] || die "release.json tag mismatch"
  printf '%s' "$(cat "${work}/release.json")" | grep -q "\"target\"[[:space:]]*:[[:space:]]*\"${target}\"" \
    || die "release.json does not list target ${target}"

  chmod 755 "${work}/${asset}"
  got_version=$("${work}/${asset}" --version 2>/dev/null | head -n 1) \
    || die "staged ocd --version failed"
  printf '%s' "${got_version}" | grep -q "${version}" \
    || die "staged binary version output does not contain ${version}: ${got_version}"

  staged="${DEST}.new.$$"
  mv "${work}/${asset}" "${staged}"
  chmod 755 "${staged}"
  # Best-effort directory sync when available.
  if command -v sync >/dev/null 2>&1; then
    sync "${staged}" 2>/dev/null || sync || true
  fi
  mv -f "${staged}" "${DEST}"
  if command -v sync >/dev/null 2>&1; then
    sync "${bin_dir}" 2>/dev/null || sync || true
  fi

  now_ms=$(date +%s)000 2>/dev/null || now_ms=0
  cat >"${RECEIPT}.tmp.$$" <<EOF
{
  "schema_version": 1,
  "version": "${version}",
  "sha256": "${actual_bin}",
  "target": "${target}",
  "binary_path": "${DEST}",
  "method": "install.sh",
  "source": "${base}/${asset}",
  "installed_at_ms": ${now_ms}
}
EOF
  mv -f "${RECEIPT}.tmp.$$" "${RECEIPT}"
  chmod 644 "${RECEIPT}"

  printf 'install.sh: installed %s -> %s\n' "${version}" "${DEST}" >&2
  printf 'install.sh: receipt %s\n' "${RECEIPT}" >&2
  printf 'install.sh: next: ocd setup --yes   # creates config/data/service; not done by install\n' >&2
}

main "$@"
