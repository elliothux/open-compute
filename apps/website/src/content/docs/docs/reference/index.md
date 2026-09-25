---
title: "Reference"
description: "Compatibility, limits, configuration, API, authentication, and runtime contracts for open-compute."
---

Use reference pages to look up stable contracts. Tutorials and operational procedures live in [Develop](/docs/develop/) and [Operate](/docs/operate/).

- [Compatibility](/docs/platform/compatibility/) — supported products, Worker APIs, Wrangler, and single-node topology
- [Behavior differences](/docs/platform/deviations/) — intentional differences from Cloudflare's hosted platform
- [Limits](/docs/platform/limits/) — configured and release-owned bounds; inspect live values with `ocd capabilities --json`
- [Not available](/docs/platform/unsupported/) — rejected or unimplemented capabilities
- [API and product index](/docs/platform/reference/api/) — product docs, management API, SDK, and generated surface authorities
- [Platform configuration](/docs/ocd/configuration/) — `compute.toml` / system config, secret references, storage, runtime, and product limits
- [Extensions](/docs/extension/) — operator-owned native Providers exposed through Service Bindings
- [CLI](/docs/cli/) — selection, output, networking, and mutation semantics

The Cloudflare-compatible management API lives under `/client/v4`. Large route and member inventories are generated from the implementation and conformance catalog instead of copied into prose.
