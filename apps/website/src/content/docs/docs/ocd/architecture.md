---
title: "How open-compute is organized"
description: "Understand the boundaries between the ocd daemon, its instances, configuration files, data directories, Gateway, and extensions."
---

open-compute separates the host-level service from the applications and data it manages. One `ocd` process can serve multiple isolated instances; each instance has its own configuration and data directory.

```text
Host
└── ocd daemon (one process for the selected user or system scope)
    ├── ocd.toml                 shared daemon settings and instance registry
    ├── Gateway                  shared listeners and TLS state
    └── registered instances
        ├── Instance A
        │   ├── compute.toml     settings for this instance
        │   ├── data directory   this instance's platform and product data
        │   └── extensions       trusted local programs started for this instance
        └── Instance B            separate config, data, and resources
```

## What each part means

| Part                    | What it does                                                                                                                                                                                                                 | Where its state is configured or stored                                                                                                   |
| ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| `ocd` daemon            | Runs the shared HTTP listeners, manages instance lifecycle, and supervises each instance's runtime. Run one daemon per selected user or system scope.                                                                        | The `ocd` executable and the selected OCD directory (`<OCD_DIR>`).                                                                        |
| `<OCD_DIR>/ocd.toml`    | Configures daemon-wide settings and explicitly lists which instances belong to the daemon. Each registry entry points to a `compute.toml` and sets `autostart`.                                                              | `<OCD_DIR>/ocd.toml`. It is not an instance's product configuration.                                                                      |
| Instance                | An isolated open-compute account: it has its own stable ID, API resources, credentials, runtime, and lifecycle. The Cloudflare-compatible API calls this identity `account_id`; the CLI and Dashboard call it `instance_id`. | Defined by one registered `compute.toml` and its configured data directory.                                                               |
| `compute.toml`          | Configures one instance, including its data location, storage, products, Dashboard, public domain, and extensions.                                                                                                           | The exact path listed in `ocd.toml`; it may be inside or outside `<OCD_DIR>`.                                                             |
| Instance data directory | Holds that instance's SQLite authority, identity, local objects (when local storage is selected), and per-instance runtime state.                                                                                            | The `[data].path` value in that instance's `compute.toml`. It is independent of the config file's location.                               |
| Gateway                 | Provides shared public ingress and TLS handling, then routes requests to the instance that claimed the domain. Daemon-wide Gateway listeners and TLS state are shared; each instance claims its own `base_domain`.           | Shared settings and persistent TLS state belong to the daemon scope; the domain claim belongs in that instance's `compute.toml`.          |
| Native extension        | Adds trusted operator-provided native functionality to one instance. It is not a separate daemon or an automatically installed package.                                                                                      | Declared under `[extensions.<name>]` in that instance's `compute.toml`; extension source and executable are operator-managed local files. |

The shared verified runtime package is cached once under `<OCD_DIR>/cache/packages/`; instance-specific runtime configuration and state remain with the instance. The master-key file is selected by `data.master_key_file` and can be outside the instance data directory, so back it up separately when it is external.

## Which operation affects what?

- Restarting `ocd` restarts the shared daemon and its managed instances.
- Starting, stopping, or restarting an instance affects only that instance. Its data and resources remain isolated from other instances.
- Removing an instance from the registry does not delete its `compute.toml` or data directory.
- An instance backup covers that instance, not the daemon registry, shared Gateway state, or other instances. Back up shared state separately.

See [Instances](/docs/ocd/instances/) for registration and lifecycle commands, [Configuration](/docs/ocd/configuration/) for TOML fields, [Gateway](/docs/gateway/) for ingress and TLS, and [Extensions](/docs/extension/) for native extensions.
