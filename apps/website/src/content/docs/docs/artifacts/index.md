---
title: "Artifacts"
description: "Git-backed artifact repositories through Wrangler, the Cloudflare-compatible API, and Worker bindings."
---

Artifacts provides account-scoped namespaces and Git-backed repositories. Use the certified Wrangler, the compatible `/client/v4` API, Git Smart HTTP, or an `artifacts` Worker binding to create, import, fork, read, and manage repositories and scoped tokens.

Repository metadata is authoritative in SQLite; bare Git data lives under the platform data directory. Tokens are returned only when created and are stored as digests. Snapshot and restore include repository files and immutable Worker Version bindings.

## Current boundary

Supported operations include namespace and repository lifecycle, public HTTPS import, independent fork, read/write token lifecycle, Git clone/fetch/push over HTTP, REST object reads, and the pinned Worker binding surface.

ArtifactFS, event subscriptions, automatic build/deploy, Git LFS, SSH, private remote import, and hosted placement are not provided. See [Products](/docs/products/) and [Compatibility](/docs/platform/compatibility/).
