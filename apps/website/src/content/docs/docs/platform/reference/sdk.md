---
title: "Management SDK"
---

`@open-compute/sdk` is the capability-scoped TypeScript SDK for the open-compute management API. It exposes exactly the operations `ocd` has qualified against the pinned official Cloudflare OpenAPI snapshot, plus open-compute-only operations under `client.openCompute`. Every standard method delegates to the pinned official [`cloudflare`](https://www.npmjs.com/package/cloudflare) SDK implementation, so authentication, retries, pagination, multipart uploads, and error parsing match the official client exactly.

## Installation

```sh
npm install @open-compute/sdk
```

Pair the SDK `X.Y.Z` with the same `ocd` release version.

## Usage

```ts
import { createOpenComputeClient } from "@open-compute/sdk";

const client = createOpenComputeClient({
  apiToken: process.env.OPEN_COMPUTE_API_TOKEN!,
  baseURL: "https://compute.example/client/v4",
});

await client.workers.scripts.versions.list("app", { account_id });
await client.d1.database.list({ account_id });
await client.openCompute.system.status();
```

## Client rules

- `apiToken` and `baseURL` are required; the client never reads ambient credential environment variables.
- `baseURL` must be an absolute URL whose canonical path ends in `/client/v4`; plain HTTP is only accepted for loopback test addresses.
- `defaultHeaders` cannot override `Authorization` or platform-internal headers.
- Construction never touches the network and never sends requests to `api.cloudflare.com`.

## Surface inventory

The exposed surface is generated from the repository OpenAPI authority and recorded in the committed surface report (`packages/sdk/surface.json` in the repository, mirrored into `release.json` for every release). Highlights:

- Workers: scripts, versions, deployments, secrets, schedules, settings, script settings, tails, assets upload, subdomain.
- Worker Script/Version upload uses one JSON `metadata` part plus named module parts and includes typed Service `props`, Artifacts bindings, and multi-step Durable Object migrations. Historical Version deletion uses the official Beta route.
- KV, D1 (including time travel), R2 objects, Queues (including metrics and message push/bulk push), Workflows, Vectorize, AI Search, memberships, user, accounts.
- Vendor operations under `client.openCompute`: capabilities, system status, scheduler pause/resume/repair, cache garbage collection, image capacity, upgrade check, worker endpoints, durable object inventory, KV and D1 backups.

Operations that `ocd` supports but the pinned official SDK does not implement are deliberately not exposed; the exclusion is recorded in the selection manifest. Deviations on supported operations are listed in [Behavior differences](/docs/platform/deviations/).

## Errors

Failures throw the official SDK error classes (`APIError` and its subclasses), re-exported from the package.
