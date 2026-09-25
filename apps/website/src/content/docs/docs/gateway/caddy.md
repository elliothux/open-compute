---
title: "Caddy configuration"
description: "Inspect the pinned Caddy runtime and safely extend the shared Gateway with operator Caddyfiles."
---

`ocd` embeds and verifies the supported Caddy build. Production does not search `PATH`, use a system Caddy, or download one at startup.

## Managed commands

```sh
ocd caddy version
ocd caddy list-modules
ocd caddy fmt ./site.Caddyfile
ocd caddy validate
ocd caddy reload
ocd caddy status
```

These commands select the current user or explicit `--system` OCD scope. They do not accept `--instance` or `--config` because one Caddy child serves the complete scoped Gateway. `fmt` prints formatted content without modifying the source file. `reload` validates and atomically applies the complete generated and imported configuration through the running daemon.

## Operator sites

Add up to 16 operator-owned standard Caddyfiles in `ocd.toml`:

```toml
[[gateway.caddy]]
caddy_file = "./sites/internal.Caddyfile"
```

Relative paths resolve against the directory containing `ocd.toml`. Duplicate files, symlinks, invalid modules, and failed validation are rejected. Operator sites do not become Worker route claims and do not create a second deployment mapping; their domains, upstreams, optional TCP 80 listener, and DNS remain operator-owned.

The generated platform routes and imported files are applied as one configuration. On failure, `ocd` keeps the last confirmed snapshot. `ocd caddy status` reports the runtime pin, child state, configuration digest, DNS state, TLS readiness, and most recent reload result without exposing Caddy storage or certificate private keys.
