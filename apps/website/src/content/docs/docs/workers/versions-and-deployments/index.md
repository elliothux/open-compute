---
title: "Versions and deployments"
---

One deploy: create or reuse a Worker → encode an immutable bundle → validate the runtime → activate (promote). Authority is local SQLite and one supervised runtime generation. The same pinned upstream Wrangler wire path serves a selected local instance or explicit remote target.

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

A failed validation does not change the current active deployment. Deploy / rollback change the active pointer; they do not mutate a ready Version's bytes.

## Compatibility

| Topic                                                                                | Cloudflare                                                                                          | open-compute                                               |
| ------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| Versions are immutable; a release switches the active pointer                        | Yes — [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | Yes                                                        |
| Rollback points at an older version instead of rewriting bytes                       | Yes                                                                                                 | Yes                                                        |
| Deploy authority                                                                     | Cloudflare global rollout / placement / traffic-splitting                                           | Local SQLite and one supervised runtime generation         |
| Gradual deployments / version affinity / Cloudflare preview URLs / Workers Builds CI | Yes                                                                                                 | Not provided                                               |
| `ocd wrangler` local target                                                          | N/A                                                                                                 | Validated local instance admin API                         |
| `ocd wrangler --target`                                                              | Wrangler deploy                                                                                     | Explicit HTTPS target; loopback HTTP is the only exception |
