---
title: "DNS and TLS"
description: "Plan DNS records, verify delegated ACME challenges, and probe a Gateway certificate and Worker route."
---

Gateway DNS changes are operator-owned. `ocd` prints and verifies a deterministic plan; it does not modify your business DNS zone.

## Required reachability

- Forward public TCP 443 to `[gateway].https_listen`.
- Forward public UDP 53 and TCP 53 to `[gateway].challenge_dns_listen`.
- The managed platform does not require TCP 80. An imported operator Caddyfile may use it separately.

`ingress_ipv4` and `ingress_ipv6` are the public addresses printed in the DNS plan, not listener bind addresses.

## Configure and verify

Select the instance whose `[public_gateway].base_domain` should be checked:

```sh
ocd --instance production config gateway-dns-plan
```

Create the emitted ingress A/AAAA records, Worker wildcard CNAME, challenge NS delegation, and any required CAA records with your DNS operator. Then run:

```sh
ocd --instance production config gateway-challenge-probe
ocd --instance production config gateway-dns-verify
ocd caddy validate
ocd caddy reload
ocd caddy status
ocd --instance production config gateway-tls-probe
```

`gateway-challenge-probe` checks direct UDP and TCP reachability of the configured challenge authority. `gateway-dns-verify` checks public recursive answers, parent delegation, CAA, and challenge authority. `gateway-tls-probe` preserves SNI and validates the managed certificate chain, private upstream marker, and Worker HTTPS route against fixed trust roots.

These probes are read-only except `ocd caddy reload`, which atomically applies the complete validated Gateway configuration. Public DNS propagation, firewall changes, and dedicated-host qualification remain operator responsibilities.
