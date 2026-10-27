---
title: "Containers"
description: "Configure an operator-owned Broker for Container execution, snapshots and directory restore."
---

Containers run through an independently operated Broker and a qualified rootful Linux Docker Engine. The daemon owns application targets and execution leases in SQLite. The Broker owns Engine access. Neither daemon startup nor tenant requests install tools, pull images or build applications.

The runtime implements the fixed 65-member Container API from Workers types `5.20260930.2`. The API includes process lifecycle, exec streams, HTTP and WebSocket transport, named images, inactivity policy, container snapshots and directory restore. Runtime compatibility and management API coverage are separate contracts.

## Prepare the provider

1. Prepare a rootful Linux Docker Engine with API 1.42 or later, cgroup v2 and seccomp.
2. Configure overlay2 on XFS with `pquota` so writable-layer disk limits take effect.
3. Prepare the pinned sidecar image, immutable application images and the verified read-only exec helper volume.
4. Create a private Broker configuration with the Engine socket, allowed daemon UID, persistent `state_dir`, spool directory and resource ceilings.
5. Run `ocd container-broker --broker-config /absolute/path/broker.toml` under the operator's service supervisor.
6. Verify the Broker qualification result and obtain the successful provider contract digest.

The Broker command rejects `--system`, `--config` and `--instance`. The scoped daemon does not supervise the Broker. A diagnostic contract with `qualified = false` does not admit Container mutations. The persistent Broker state is not disposable cache.

If a qualification check returns an error, the Broker exits instead of serving a contract. A sidecar qualification failure records its fixed phase and whether the overall deadline expired. The failure event contains no credentials or upstream errors.

The [repository operator guide](https://github.com/elliothux/open-compute/blob/main/docs/references/containers.md) specifies the complete Broker configuration, helper preparation and qualification checks. An installed Docker CLI or an accepted Engine configuration alone does not establish qualification.

## Instance configuration

Add the following sections to the selected instance's `compute.toml`. Replace the placeholder with the digest from that provider's successful qualification. Omit both sections to keep Containers disabled.

```toml
[containers]
broker_socket = "/var/lib/open-compute/run/container-broker.sock"
provider_contract_sha256 = "<qualified-contract-sha256>"
max_instances = 4
max_snapshots = 32
max_snapshot_bytes = 8589934592

[containers.resources]
cpuNanos = 1000000000
memoryBytes = 1073741824
diskBytes = 4000000000
pids = 128
openFiles = 1024
```

The socket must be an absolute private Unix socket path. Instance ceilings must not exceed the qualified provider ceilings. A missing provider, mismatched contract digest or unsupported native protocol rejects Container admission. Other Worker entry points keep their existing configuration.

## Deploy and inspect

Use the pinned project-local cf, the official Cloudflare Vite plugin and `@cloudflare/containers`. Declare Container classes and images in the application configuration. Build applications before upload; the daemon validates uploaded modules and immutable image references.

```sh
ocd cf deploy --mode production
ocd --instance dev capabilities --json
```

Code-only deployment preserves the current execution. An image or resource change creates a new immutable target. Replacement waits for positive cleanup evidence from the previous execution. Unknown outcomes retain the execution lease and consume capacity.

## Management boundary

The implemented `/client/v4/accounts/{account_id}/containers` subset provides:

- Application list, create, get, patch and delete; account limits through `/me`.
- Image preparation and registry credentials for the fixed cf deployment flow.
- Rollout list, create and get.
- Instance list through `instances-v2` and individual instance observations.

The route authority is `openapi/cloudflare-subset-manifest.json` under `clientObservedOperations`. `ocd capabilities --json` projects this inventory. These observed cf routes are outside the pinned upstream OpenAPI subset. The generated `@open-compute/sdk` facade has no Container management methods because the pinned official SDK has none; the generated surface report records the exclusions.

Only immediate rollout and the implemented rolling steps are available. `rollout_active_grace_period` must be zero. A running rollout rejects a new target with HTTP 409; the platform can reuse an unchanged physical target. Application `observability.logs` retains configuration intent but does not collect the main process stdout or stderr. The Dashboard does not provide a separate Container management page.

## Snapshots and accepted differences

Container snapshots retain filesystem data and exclude volatile `/run` data and injected helper mounts. Restore creates a new execution. Directory restore verifies the archive and restores paths before the main process starts. The local `snapshotDirectory()` producer remains experimental; the nine stable Directory fields still have a real local restore path.

Two snapshot behaviors follow the accepted upstream direction: `snapshotContainer()` accepts omitted options, and a restored container returns an empty `inspect().image`. The platform does not invent a Cloudflare-managed registry reference or claim the hosted image-size formula. The [0.4.0 release notes](https://github.com/elliothux/open-compute/blob/main/docs/releases/0.4.0.md) record the upstream sources, candidate checks and remaining formal publication checks.

Operators own host network filtering. The platform does not claim per-Container virtual machines, multi-region placement or isolation between Worker and instance IP networks. Engine sockets, private callbacks and management APIs retain independent authentication.
