---
title: "Dashboard"
description: "Enable the operator Dashboard, open a one-time login, and switch between registered instances."
---

The Dashboard is bundled with `ocd` and uses the same `/client/v4` API as Wrangler and the official SDK. Enable it in each instance that should serve the operator UI:

```toml
[dashboard]
enabled = true
```

`ocd setup` enables it for the first instance by default. No separate Dashboard process or package is required.

## Open the Dashboard

With one registered instance:

```sh
ocd dashboard
```

With multiple instances, select the ready instance that should issue the login:

```sh
ocd --instance staging dashboard
```

Use `--no-open` to print the URL or `--json` for versioned machine output. The URL contains a short-lived, one-time login code; it does not expose a long-lived admin token. After exchange, the browser receives a bounded operator session.

## Switch instances

The account switcher lists instances registered in the same selected daemon scope. In Dashboard terminology, one account is one open-compute instance: its InstanceId, data directory, SQLite authority, object authority, credentials, resources, and workerd remain isolated from every other entry.

Switching accounts changes the selected API authority; it does not merge resources, credentials, or caches between instances. Use `ocd instance start|stop|restart <id-or-name>` for instance lifecycle operations.

The operator surface is available only on accepted loopback operator hosts and remains protected by the daemon admin authority or a valid browser session. See [Instances](/docs/ocd/instances/) and [Health checks](/docs/ocd/health/).
