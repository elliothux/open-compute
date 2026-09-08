# open-compute 0.1.2

0.1.2 is the first release with a complete day-to-day operator workflow for a single-machine open-compute installation. You can now initialize an instance, run it as an OS service, inspect it, open its Dashboard, upgrade the installed binary, and uninstall it without assembling those steps by hand.

This release is intended for individual operators and small teams running one `ocd` process and its supervised `workerd` child on one machine.

## What's new

- `ocd setup` creates a configuration, protected token files, an instance registry entry, and a managed service. Use `ocd setup --yes` to accept the recommended defaults.
- `ocd instances`, `start`, `stop`, `restart`, `status`, `logs`, and `instance remove` provide a consistent lifecycle for named local instances. Explicit selectors fail instead of silently choosing the wrong instance.
- systemd and launchd integration installs and controls the instance service, then waits for both the local control socket and HTTP readiness check.
- `ocd dashboard` creates a one-time local login URL. The browser exchanges it for a short-lived session instead of storing the long-lived admin token.
- Install receipts, release checks, `ocd upgrade`, and `ocd uninstall` now cover the binary lifecycle. Upgrade verifies the release manifest and checksums, replaces only a binary owned by an open-compute receipt, and restarts only instances that were running before the upgrade.

## Fixed

- Instance control sockets no longer inherit an arbitrarily long macOS `TMPDIR`. When `XDG_RUNTIME_DIR` is unavailable, open-compute uses the bounded, user-specific `/tmp/open-compute-<uid>` directory. This fixes `ocd start` and setup failures caused by macOS Unix-socket path limits.
- Database migrations shipped in a release are now immutable. Future schema changes must add a migration instead of editing an already released migration.

## Before you upgrade

- No data reset or configuration rewrite is required for 0.1.2.
- 0.1.1 does not include `ocd upgrade`. Stop its managed service, install the 0.1.2 binary, and start the service again. Installations created by `scripts/install.sh` in 0.1.2 can use `ocd upgrade` for later releases.
- `ocd uninstall` and `ocd instance remove` preserve configuration, secrets, and workload data. Removing that retained data is a separate, explicit operator action.
- open-compute is pre-1.0. A future release may announce a breaking persisted-data change, but this release does not introduce one.

After replacement, verify the installation with:

```console
ocd --version
ocd config check
ocd doctor
```

## Install or upgrade

For a new installation, download and review the installer, then pin this release explicitly:

```console
curl -fsSL -o install.sh https://raw.githubusercontent.com/elliothux/open-compute/v0.1.2/scripts/install.sh
OPEN_COMPUTE_RELEASE_TAG=v0.1.2 sh install.sh
ocd setup --yes
```

The installer downloads from GitHub Releases, verifies `release.json` and `SHA256SUMS`, and refuses to overwrite a binary it does not own. It does not create configuration, data, credentials, or a service; `ocd setup` performs that step.

For an existing manual or 0.1.1 installation, follow the [install and first-start runbook](https://github.com/elliothux/open-compute/blob/v0.1.2/docs/references/runbooks/install-and-first-start.md) and verify the downloaded binary against `SHA256SUMS` before replacing it.

## Downloads

| Host | Asset |
| --- | --- |
| macOS on Apple silicon | `ocd-v0.1.2-darwin-arm64` |
| Linux on ARM64 | `ocd-v0.1.2-linux-arm64` |
| Linux on x86-64 | `ocd-v0.1.2-linux-x64` |

The release also includes `release.json` and `SHA256SUMS`. The executable embeds the pinned `v1.20260905.0-open-compute-p1.b3e1a278` runtime (`workerd 2026-09-05`); no separate runtime download is needed at startup.

Windows and Intel macOS do not have qualified 0.1.2 binaries.

## Security

There are no published security advisories specific to 0.1.2. The new Dashboard login flow reduces exposure of the long-lived admin token, and binary upgrades fail closed on manifest, checksum, ownership, or version mismatches.

## Known limitations

- Interactive setup currently provisions local object storage. Configure S3-compatible storage directly in TOML when required.
- The release qualification covers the packaged executable and service-generation behavior, not every host distribution or local systemd/launchd policy.
- Code signing and macOS notarization are not included in 0.1.2.
- Hosted Cloudflare differential testing, broad third-party application qualification, and long-running soak tests are separate from this release qualification.

## Verification

Release revision [`ff57e5bbadf0c83b65eb51d399192c472a729163`](https://github.com/elliothux/open-compute/commit/ff57e5bbadf0c83b65eb51d399192c472a729163) passed the Linux and macOS release workflow, including Rust 1.98 compatibility, linting, at least 90% Rust line coverage, workspace Gates, the controlled Linux egress fixture, native single-executable packaging, and post-upload checksum verification.

[Full changelog](https://github.com/elliothux/open-compute/compare/v0.1.1...v0.1.2) · [Release workflow](https://github.com/elliothux/open-compute/actions/runs/34189528035) · [Operator experience design](https://github.com/elliothux/open-compute/blob/v0.1.2/docs/implemented/p11-ocd-operator-experience.md)
