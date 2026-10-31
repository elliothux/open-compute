---
title: "ocd CLI"
description: "Task-oriented guide to the open-compute daemon, developer launcher, diagnostics, and maintenance commands."
---

`ocd --help` and each subcommand's `--help` output are the parameter authority. This page groups commands by task and explains their shared behavior.

## Install and run

- `setup`, `run`
- `start`, `stop`, `restart`, `status`, `logs`
- `instances`, `instance setup|add|start|stop|restart|remove`, `purge`
- `dashboard`

`ocd run` starts the selected OCD scope: the current user's scope by default, or `ocd run --system`. It reads only that scope's `ocd.toml` and does not accept `--config` or `--instance`. `ocd start|stop|restart|status|logs|instances` operate on that one scoped daemon service and reject both instance selectors.

`ocd caddy version|list-modules|fmt|validate|reload|status` also operates on the selected OCD scope and rejects `--config` and `--instance`; it manages the shared Gateway, not an instance Gateway. `reload` atomically applies the complete Caddy configuration through the running scoped daemon.

Managed registrations come only from `<OCD_DIR>/ocd.toml`. Each entry stores `config` and `autostart`; `instances/` is not scanned, and `[data].path` in that exact config is the sole data-root authority.

Instance-scoped commands select a registered instance with `--instance` or read the exact file supplied by `--config`. Without either, they select the sole registered instance in the chosen OCD scope; zero or multiple registrations require an explicit choice. The CLI does not discover a config from the working directory, HOME/XDG, or `/etc/open-compute`.

For example, `ocd restart` restarts the scoped daemon and all of its instances, while `ocd instance restart staging` restarts only that instance. Use `ocd --instance staging dashboard` to open its Dashboard. See [Instances](/docs/ocd/instances/), [Dashboard](/docs/ocd/dashboard/), and [Gateway](/docs/gateway/).

## Develop and deploy

- `target add|list|show|test|remove`
- `cf [--target <name>] [--project <dir>] <cf-command> ...`

`ocd cf` resolves the project-local cf and passes every argument from the cf command onward unchanged. It never downloads, repairs, or silently replaces the dependency. Versions outside `1.0.x` warn but still execute; internal CI pins `1.0.0-beta.12`.

The target registry is stored at `<OCD_DIR>/targets.toml` for the selected user or explicit system scope. Update-check metadata is a disposable cache at `<OCD_DIR>/cache/update-check.json`.
`ocd target add <name> --api-base-url <url> --instance-id <id> --token-file <path>` records the remote open-compute InstanceId. The same value appears as `account_id` only on Cloudflare-compatible API and cf surfaces.

## Inspect and diagnose

- `config init|check`
- `config gateway-dns-plan|gateway-challenge-probe|gateway-dns-verify|gateway-tls-probe`
- `doctor`, `capabilities`, `support-bundle`
- `docs`, `licenses`

Read-only commands support `--json` where the command help advertises it. JSON is versioned; human output is optimized for operators and is not a parsing contract.

## Maintain and recover

- `backup create|list|inspect|delete|retention-plan|restore`
- `backup cleanup-incomplete|cleanup-restore|attest-restore-smoke`
- `scheduler recover-corrupt` (refuses replacement of shared `control.sqlite`; use a full snapshot restore)
- `cache clean [--instance <id-or-name>|--all] [--dry-run]`
- `upgrade`, `uninstall [--purge --yes]`, `purge --instance <id>|--config <path>`

Backup and scheduler recovery commands require the offline or exclusive conditions stated in their help and the [operator guide](/docs/operate/). Do not edit SQLite files or migration tables directly.

`ocd cache clean` removes unpinned, regenerable entries from the selected OCD scope's shared cache only. `--instance <id-or-name>` selects one registered instance cache; `--all` covers both the shared cache and every registered instance. `--instance` and `--all` are exclusive. `--dry-run` previews without creating directories or writing data. A running daemon performs cleanup through its owner-only control socket. Offline cleanup requires the existing OCD lock, instance locks, and child-process recovery; an unreachable socket is never taken as proof that the daemon stopped. Reports distinguish freed or eligible bytes, skipped entries, and failures. It does not purge data, clear temporary recovery state, or remove the active embedded runtime package.

`instance add --config <path>` registers an initialized config with the running scoped daemon without rewriting its `[data].path`. `instance remove <id-or-name>` stops that instance and removes only its manifest entry; the config and data remain. These online changes use the owner-only daemon socket, and an external edit to `ocd.toml` makes the next online write fail instead of overwriting it. `uninstall` preserves instance data unless `--purge` is explicit. `purge` requires one exact selector, prints its plan, supports `--dry-run`, and requires `--yes` when stdin is not interactive.

`ocd instance setup --name dev --yes` creates a fresh instance through the running daemon. `--config` and `--data-dir` independently choose its configuration and explicit `[data].path`; `--autostart=false` and `--start=false` disable the two default startup choices. Without `--yes`, an interactive confirmation is required. Existing configuration or nonempty unknown data is never overwritten.

Successful `ocd cf` execution replaces the launcher process, so cf owns the final stdout, stderr, signals, and exit status.

## Container Broker

`ocd container-broker --broker-config /absolute/path/broker.toml` runs the independent Broker in the foreground. The command rejects `--system`, `--config` and `--instance`. The operator owns its service and Engine; the scoped daemon does not supervise it. See [Containers](/docs/containers/).
