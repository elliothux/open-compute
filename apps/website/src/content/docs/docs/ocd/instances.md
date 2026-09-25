---
title: "Instances"
description: "Register and operate isolated open-compute instances under one scoped ocd daemon."
---

One `ocd` daemon owns the selected user or system scope: its shared listeners, Gateway, scope lock, and instance registry. Each registered instance separately owns its exact `compute.toml`, `[data].path`, InstanceId, SQLite authority, object authority, credentials, cache, and supervised workerd and Provider processes.

The Cloudflare-compatible `/client/v4` wire calls this identity `account_id`. The `ocd` CLI, Dashboard, and private open-compute interfaces call it `instance_id`; they refer to the same authority.

## Registry

`<OCD_DIR>/ocd.toml` is the only managed registry. It stores no copied identity or data path:

```toml
[[instances]]
config = "instances/default/compute.toml"
autostart = true

[[instances]]
config = "/srv/open-compute/staging/compute.toml"
autostart = false
```

The daemon never scans `instances/`. Identity and data location are loaded from the exact registered configuration and its SQLite authority. Registered data roots cannot overlap.

## Create and register

The first-host workflow creates the daemon scope and its first instance:

```sh
ocd setup --yes
```

Create another instance through the running daemon:

```sh
ocd instance setup --name staging --yes
```

Use `--config` and `--data-dir` to choose independent absolute locations. `--autostart=false` and `--start=false` disable those defaults. To register an already initialized configuration without rewriting it:

```sh
ocd --config /srv/open-compute/staging/compute.toml instance add
```

## Operate

```sh
ocd instances
ocd instance start staging
ocd instance stop staging
ocd instance restart staging
ocd instance remove staging
```

`start`, `stop`, and `restart` do not change persisted `autostart`. `remove` stops the instance and removes only its registry entry; configuration and data remain.

Instance-scoped commands accept `--instance <id-or-name>` or an exact `--config <path>`. With neither selector, they use the sole registered instance; zero or multiple registrations require an explicit selector. Daemon commands such as `ocd start`, `stop`, `restart`, `status`, and `logs` operate on the whole selected scope.

See [Architecture and boundaries](/docs/ocd/architecture/), [Configuration](/docs/ocd/configuration/), [Dashboard](/docs/ocd/dashboard/), and the [CLI guide](/docs/cli/).
