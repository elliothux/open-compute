# CLI reference

Trust `ocd --help` and the current binary. Global selectors:

- `--config <path>` — exact configuration path; a relative value resolves only against the startup working directory and is never searched from parent directories or `$HOME`. Paths inside the file resolve against its canonical directory.
- `--instance <id>` — exact registered short instance ID (mutually exclusive with `--config`).
- `--no-update-check` — skip upgrade reminder and asynchronous update-check refresh for this invocation.

`--instance` and `--config` are rejected together before any file or registry access.

## Config discovery

When a command needs configuration and neither `--config` nor `--instance` is set, discovery order is:

1. exact `./compute.toml` in the startup working directory;
2. `/etc/open-compute/config.toml`;
3. otherwise fail with the checked paths and a hint to run `ocd setup`.

A present but unloadable higher-priority file fails closed; it is never shadowed by a lower-priority path. `ocd run` does not accept `--instance`.

Global commands (no ordinary config discovery): `--help`, `--version`, `docs`, `licenses`, `instances`, `target`, `setup`, `upgrade`, `uninstall`, `worker bundle`. `wrangler` uses its own exact local-instance or remote-target selection.

## `instances`

List registered local instances (system + current-user registries). JSON includes `instance_id`, `state`, `config`, and `service`. Without a control socket (P11.2), listed state is `stopped` even if a foreground `ocd run` is active.

```sh
ocd instances
ocd instances --json
```

## `target`

Manage explicit per-user remote Wrangler targets. Add validates a strict target name, normalized HTTPS `/client/v4` URL (loopback HTTP only), canonical account ID, and an absolute owner-only `0600` deployer-token file. The registry stores the file reference, never the token value.

```sh
ocd target add company-prod \
  --api-base-url https://compute.example.com/client/v4 \
  --account-id 0123456789abcdef0123456789abcdef \
  --token-file /absolute/path/deployer.token
ocd target list [--json]
ocd target show company-prod [--json]
ocd target test company-prod [--json]
ocd target remove company-prod
```

Only `test` makes a network request and opens the token file. Remove keeps the external token file.

## `wrangler`

Select an open-compute authority, read its capability-advertised certified Wrangler version, and replace `ocd` with the nearest project-local Wrangler. Minor and patch drift is accepted silently; a major mismatch warns without blocking the child command. Arguments beginning with the Wrangler command are passed unchanged.

```sh
ocd wrangler deploy --env dev
ocd --instance k7m2r wrangler tail --env staging
ocd wrangler --target company-prod --project /srv/workers/api deploy --env production
ocd wrangler -- --version
```

`--target`, global `--instance`, and global `--config` are mutually exclusive. `--project` sets both the executable-search root and child working directory. Without `--project`, both start at the invocation directory. A successful launcher preserves the TTY, signals, stdout/stderr, and Wrangler exit status. See [Wrangler projects and deployment targets](/workers/projects).

## `docs`

List or print an operator runbook embedded in the executable. Repository path changes do not rename these manuals.

```sh
ocd docs
ocd docs install-and-first-start
```

Names (no `.md`): `backup-and-retention`, `collect-support-bundle`, `disk-pressure`, `fresh-host-restore`, `install-and-first-start`, `master-key-loss-and-recovery`, `s3-outage`, `scheduler-recovery`, `sqlite-corruption`, `current-release-recovery`, `workerd-crash-loop`.

Site pages are the operator-facing prose; `ocd docs` prints the embedded runbooks. Commands should match. If a runbook example uses `platform.toml`, still pass your absolute `--config` path.

## `licenses`

Print licenses included in this executable (Open Compute and embedded Cloudflare workerd).

```sh
ocd licenses
```

## `capabilities`

Print the versioned product and release contract. Uses config discovery or `--config` / `--instance`. `--json` emits `schema_version`, `release`, `runtime`, `products`, `limits`. How to read it: [Compatibility](/platform/compatibility).

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

## `config init` / `config check`

```sh
ocd config init --data-dir /var/lib/open-compute
ocd config check
ocd --config /etc/open-compute/config.toml config check --json
```

`init`: a relative `--data-dir` is resolved against the startup working directory and emitted as an absolute path; a complete starter TOML goes to stdout; no files or secrets are created. A successful JSON check looks like `{"schema_version":1,"command":"config_check","result":"ok"}`; human output is `CONFIG_OK`. `check` uses config discovery when `--config` is omitted.

## `run`

Start the platform process in the foreground. First run generates identity, databases, and the master key after taking the lock, then materializes the embedded runtime. Does not accept `--instance`.

```sh
ocd --config /etc/open-compute/config.toml run
ocd run   # uses ./compute.toml or /etc/open-compute/config.toml
```

## `doctor`

Default is read-only. `--full` authorizes a selected object-authority canary and a temporary workerd compile/start/stop. `--json` emits a versioned report. See [Health checks](/ocd/health). Uses config discovery when `--config` is omitted.

```sh
ocd --config /etc/open-compute/config.toml doctor --json
ocd --config /etc/open-compute/config.toml doctor --full --json
```

## `backup`

Offline full-platform snapshots.

| Command                                                                    | Role                                                                      |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `backup create --name <label>`                                             | Create and fully verify a committed snapshot                              |
| `backup list`                                                              | List authenticated committed snapshots for this platform                  |
| `backup inspect --snapshot <uuid> [--verify]`                              | Inspect one; `--verify` hashes every object                               |
| `backup delete --snapshot <uuid>`                                          | Delete that snapshot's owned objects; manifest last                       |
| `backup retention-plan --keep-last <n> [--max-age-seconds] [--keep-label]` | Plan only; no deletes                                                     |
| `backup cleanup-incomplete`                                                | Remove incomplete uploads older than grace                                |
| `backup restore --snapshot <uuid>`                                         | Restore into an **empty** new data-dir                                    |
| `backup cleanup-restore --staging <uuid>`                                  | Exact staging cleanup from a failure receipt                              |
| `backup attest-restore-smoke --snapshot <uuid> --passed`                   | Record that product smoke passed; does not replace actually running smoke |

Uses config discovery or `--config` / `--instance`; all accept `--json`. Procedures: [Backup and retention](/ocd/backup) and the [incident handbook](/ocd/incidents/).

## `setup`

First-host initialization. Creates config and `0600` Bearer token files; when service start is selected, it also registers the instance and starts the managed service. Refuses overwrite. Does not require prior config discovery.

```sh
ocd setup --yes
ocd setup --config ./compute.toml --yes
ocd setup   # TTY prompts; non-TTY requires --yes
```

`--yes` without `--config` uses system defaults: `/etc/open-compute/config.toml`, `/var/lib/open-compute`, local objects, loopback listeners, Dashboard enabled. Run it through `sudo` from the non-root account that should own the service; the generated system service runs as that account. Project-local `--config ./compute.toml --yes` uses user scope and data-dir `./.data/open-compute`. Interactive setup currently supports local object storage; configure S3 explicitly in TOML. Master key path is configured but not pre-written; first `ocd run` generates it.

## `start` / `stop` / `restart` / `status` / `logs` / `dashboard`

Managed OS service lifecycle (systemd on Linux, launchd on macOS). Selectors follow online rules: `--instance`, `--config`, or the single running instance.

```sh
ocd start --config /etc/open-compute/config.toml
ocd status --json
ocd stop --instance k7m2r
ocd logs --instance k7m2r
ocd dashboard --instance k7m2r
ocd instance remove --instance k7m2r   # requires stopped; keeps config and data
```

`start` registers the instance when needed, installs the unit/plist, enables, and starts it. A registered instance keeps its persisted ID and service scope. Start/restart succeeds only after the live control socket and `/health/ready` both report ready. Foreground `ocd run` publishes the same generation descriptor and control socket for status/stop without going through the service manager.

`ocd dashboard` requests a one-time login code over the control socket and opens `/operator/#login=<code>` (or prints it with `--no-open`). The Dashboard exchanges the code for a short-lived browser session; long-lived admin tokens are not stored in `sessionStorage`.

## `upgrade` / `uninstall`

Formal binary lifecycle. Downloads come from GitHub Releases (`elliothux/open-compute`); `scripts/install.sh` writes a secret-free install receipt under `$PREFIX/share/open-compute/install-receipt.json`.

```sh
ocd upgrade --dry-run
ocd upgrade                 # latest stable
ocd upgrade 0.1.1           # exact stable SemVer
ocd upgrade --no-restart    # replace binary; leave managed instances on the old inode until restarted
ocd uninstall               # receipt-owned binary + receipt only; refuses registered instances
```

`upgrade` rejects downgrades, prereleases, checksum mismatches, changed receipt-owned binary bytes, and package-manager-owned installs. Before replacing the binary it validates every registered config and records which instances are active. It restarts only those active instances, waits for control-socket + HTTP readiness and the target release version, and leaves stopped instances stopped. A restart failure stops further restarts and keeps diagnostics. `uninstall` never deletes config, secrets, or data.

Management CLI invocations may print a one-line stderr update reminder from a user cache and spawn a detached `ocd __update_check` helper when the cache is stale. `ocd run` never refreshes over the network. Dashboard Platform can check for a strictly newer stable release and shows the host-side `ocd upgrade [version]` command; it does not execute or poll upgrades.
