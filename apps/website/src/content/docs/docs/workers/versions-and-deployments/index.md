---
title: "Versions and deployments"
---

One deploy: create or reuse a Worker → encode an immutable bundle → validate the runtime → activate (promote). Authority is local SQLite and one supervised runtime generation. The same pinned upstream Wrangler wire path serves a selected local instance or explicit remote target.

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

A failed validation does not create or change the current active deployment. Before activation, the exact Version must load in the currently running workerd generation; a generation change between validation and commit rejects the deployment. Deploy / rollback change the active pointer; they do not mutate a ready Version's bytes.

The management SDK exposes Cloudflare's Beta Worker Version DELETE route for historical cleanup. Deleting a non-active Version tombstones only that immutable Version and releases its binding references; it never deletes the current Worker or external KV, D1, R2, Queue, Workflow, or Durable Object data. Active, pinned, persistently referenced, ambiguous-prefix, and cross-instance targets fail closed. Repeating a completed delete is safe, but the removed Version can no longer be rolled back to.

The Beta Worker GET prerequisite used by the pinned Wrangler returns the current Worker identity while reporting `workers.dev` and preview subdomains as disabled; open-compute does not synthesize hosted Cloudflare DNS.

Each committed deployment has a mutable runtime assessment separate from its immutable bytes. An exactly attributed unexpected workerd exit quarantines that deployment and atomically falls back to the newest older dispatchable deployment. Ambiguous concurrent incidents do not guess a culprit. `GET /client/v4/open-compute/system/status` exposes dispatchable/quarantined counts and `active_runtime_dispatchable`; the latter is false while the runtime health component is unavailable. A support bundle includes `deployment-runtime.json` and, after an incident, bounded redacted `workerd-last-exit.json`.

## Compatibility

| Topic                                                                                | Cloudflare                                                                                          | open-compute                                               |
| ------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| Versions are immutable; a release switches the active pointer                        | Yes — [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | Yes                                                        |
| Rollback points at an older version instead of rewriting bytes                       | Yes                                                                                                 | Yes                                                        |
| Deploy authority                                                                     | Cloudflare global rollout / placement / traffic-splitting                                           | Local SQLite and one supervised runtime generation         |
| Gradual deployments / version affinity / Cloudflare preview URLs / Workers Builds CI | Yes                                                                                                 | Not provided                                               |
| `ocd wrangler` local target                                                          | N/A                                                                                                 | Validated local instance admin API                         |
| `ocd wrangler --target`                                                              | Wrangler deploy                                                                                     | Explicit HTTPS target; loopback HTTP is the only exception |
