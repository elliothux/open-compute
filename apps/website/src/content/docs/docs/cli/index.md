---
title: "ocd CLI"
description: "Task-oriented guide to the open-compute daemon, developer launcher, diagnostics, and maintenance commands."
---

`ocd --help` and each subcommand's `--help` output are the parameter authority. This page groups commands by task and explains their shared behavior.

## Install and run

- `setup`, `run`
- `start`, `stop`, `restart`, `status`, `logs`, `dashboard`
- `instances`, `instance remove`

Use global `--config <path>` or `--instance <id>` to select a local instance. Most online commands can select the only running instance automatically; zero or multiple eligible instances fail closed.

## Develop and deploy

- `target add|list|show|test|remove`
- `wrangler [--target <name>] [--project <dir>] <wrangler-command> ...`
- `worker bundle`

`ocd wrangler` resolves the project-local Wrangler and passes every argument from the Wrangler command onward unchanged. It never downloads, repairs, or silently replaces the dependency. A different Wrangler major produces a warning but does not block execution.

## Inspect and diagnose

- `config init|check`
- `doctor`, `capabilities`, `support-bundle`
- `docs`, `licenses`

Read-only commands support `--json` where the command help advertises it. JSON is versioned; human output is optimized for operators and is not a parsing contract.

## Maintain and recover

- `backup create|list|inspect|delete|retention-plan|restore`
- `backup cleanup-incomplete|cleanup-restore|attest-restore-smoke`
- `scheduler recover-corrupt`
- `upgrade`, `uninstall`

Backup and scheduler recovery commands require the offline or exclusive conditions stated in their help and the [operator guide](/docs/operate/). Do not edit SQLite files or migration tables directly.

Successful `ocd wrangler` execution replaces the launcher process, so Wrangler owns the final stdout, stderr, signals, and exit status.
