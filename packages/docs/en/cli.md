# CLI reference

Trust `ocd --help` and the current binary. `--config` is global and must be an absolute path; it is never searched from cwd or `$HOME`. Flags below are from the CLI source; none are invented.

No config required: `--help`, `--version`, `docs`, `licenses`, `capabilities`, `config init`, `worker bundle`. Every other subcommand needs `--config`.

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

Print the versioned product and release contract. `--json` emits `schema_version`, `release`, `runtime`, `products`, `limits`. How to read it: [Capabilities and limits](/en/capabilities).

```sh
ocd capabilities --json
ocd --config /etc/open-compute/config.toml capabilities --json
```

## `config init` / `config check`

```sh
ocd config init --data-dir /var/lib/open-compute
ocd --config /etc/open-compute/config.toml config check
ocd --config /etc/open-compute/config.toml config check --json
```

`init`: `--data-dir` is absolute; a complete starter TOML goes to stdout; no files or secrets are created. A successful JSON check looks like `{"schema_version":1,"command":"config_check","result":"ok"}`; human output is `CONFIG_OK`.

## `run`

Start the platform process. First run generates identity, databases, and the master key after taking the lock, then materializes the embedded runtime.

```sh
ocd --config /etc/open-compute/config.toml run
```

## `doctor`

Default is read-only. `--full` authorizes an S3 canary and a temporary workerd compile/start/stop. `--json` emits a versioned report. See [Health checks](/en/health).

```sh
ocd --config /etc/open-compute/config.toml doctor --json
ocd --config /etc/open-compute/config.toml doctor --full --json
```

## `backup`

Offline full-platform snapshots.

| Command | Role |
| --- | --- |
| `backup create --name <label>` | Create and fully verify a committed snapshot |
| `backup list` | List authenticated committed snapshots for this platform |
| `backup inspect --snapshot <uuid> [--verify]` | Inspect one; `--verify` hashes every object |
| `backup delete --snapshot <uuid>` | Delete that snapshot's owned objects; manifest last |
| `backup retention-plan --keep-last <n> [--max-age-seconds] [--keep-label]` | Plan only; no deletes |
| `backup cleanup-incomplete` | Remove incomplete uploads older than grace |
| `backup restore --snapshot <uuid>` | Restore into an **empty** new data-dir |
| `backup cleanup-restore --staging <uuid>` | Exact staging cleanup from a failure receipt |
| `backup attest-restore-smoke --snapshot <uuid> --passed` | Record that product smoke passed; does not replace actually running smoke |

All require `--config`; all accept `--json`. Procedures: [Backup and retention](/en/backup) and the [incident handbook](/en/incidents/).

## Commands used during incidents

```sh
ocd --config /etc/open-compute/config.toml support-bundle --output /var/tmp/open-compute-support-20260826.tar --json
ocd --config /etc/open-compute/config.toml scheduler recover-corrupt --backup-name scheduler-corrupt-20260826
```

`support-bundle`: `--output` must be an absolute path that does not exist and is not a symlink. `scheduler recover-corrupt` is only for an alarm-only directory whose control plane is verifiable and that has no Queue/Cron/Workflow authority. Full stop conditions: [incident handbook](/en/incidents/).

`ocd worker bundle` is an offline developer tool: versioned build JSON on stdin, canonical bundle on stdout. It is not part of installing the daemon; this site does not cover the Worker programming API.
