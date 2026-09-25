---
title: "Gateway"
description: "Expose registered instances through one shared, managed Caddy Gateway with explicit DNS and TLS authority."
---

The optional Gateway adds public HTTPS origins without changing each Worker's `.localhost` origin. One scoped `ocd` daemon owns the shared, pinned Caddy child, challenge DNS provider, listeners, configuration state, and routes for all registered instances.

Gateway configuration has two authorities:

- `<OCD_DIR>/ocd.toml` owns shared listeners, ingress addresses, trusted PROXY peers, and operator Caddyfiles.
- Each instance's `compute.toml` optionally owns one exclusive `base_domain`.

```toml title="ocd.toml"
[gateway]
ingress_ipv4 = ["203.0.113.10"]
https_listen = "0.0.0.0:8443"
challenge_dns_listen = "0.0.0.0:8053"
```

```toml title="compute.toml"
[public_gateway]
base_domain = "compute.example.com"
```

A Worker named `api` then keeps its local origin and may receive `https://api.compute.example.com`. R2 uses the fixed `r2.<base_domain>` namespace when that product is enabled. Registered instances cannot claim equal, parent, or child base domains.

## Failure boundary

`ocd` generates the complete managed Caddy configuration, validates it, and reloads atomically. A rejected reload keeps the last confirmed configuration. Caddy crash recovery is supervised and bounded; public Gateway failure does not delete the local Worker route or its persisted claim.

Certificate and ACME authority lives below `<OCD_DIR>/gateway/` and is shared daemon state, not part of an instance snapshot. Back it up with `ocd.toml`, global keys, and any external operator Caddyfiles.

Continue with [DNS and TLS](/docs/gateway/dns-tls/) and [Caddy configuration](/docs/gateway/caddy/). Public DNS and certificate qualification requires a host reachable on TCP 443 and UDP/TCP 53.
