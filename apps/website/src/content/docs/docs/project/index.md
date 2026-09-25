---
title: "Project"
description: "Architecture, source build, testing, workerd, security, contribution, and release guidance."
---

open-compute is a single-daemon Rust platform. One scoped `ocd` process owns shared listeners, the Gateway, the instance registry, and the scope lock. Each registered instance owns its configuration, data-directory lock, SQLite and object authority, scheduling, credentials, and supervised pinned workerd child.

## Architecture

Lower-level crates own core types, storage, artifacts, and runtime supervision. Workers owns immutable bundles, deployments, routes, and runtime-source snapshots. Service is the composition root. Dependency boundaries are enforced in CI.

The current formal runtime is the verified `elliothux/workerd` fork pinned by `packages/runtime/workerd.lock.json` and shipped inside each release binary. Production startup is offline and never searches `PATH` or downloads a runtime.

## Build and contribute

Repository development requires the pinned Rust, Bun, TypeScript, and Git LFS inputs. Build runtime assets explicitly before Cargo consumes them. The project uses one complete final Gate round and keeps Rust line coverage at or above 90%.

- [Repository architecture](https://github.com/elliothux/open-compute/blob/main/AGENTS.md)
- [Build and single-binary guide](https://github.com/elliothux/open-compute/blob/main/docs/references/single-binary.md)
- [Testing policy](https://github.com/elliothux/open-compute/blob/main/docs/references/testing.md)
- [workerd fork and pin](https://github.com/elliothux/open-compute/blob/main/docs/workerd/README.md)
- [Release process](https://github.com/elliothux/open-compute/blob/main/docs/references/releasing.md)

Keep user installation and application development in the corresponding docs areas; source-build instructions are contributor workflows, not prerequisites for running a release.
