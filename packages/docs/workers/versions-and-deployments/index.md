# Versions and deployments

One deploy: create or reuse a Worker → encode an immutable bundle → validate the runtime → activate (promote). Authority is local SQLite and one supervised runtime generation. `oc run` is this path locally; `oc deploy` is remote HTTPS.

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

A failed validation does not change the current active deployment. Promotion / rollback change the active pointer; they do not mutate a ready deployment's bytes. Preview is the local URL printed by `oc run`.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Versions are immutable; a release switches the active pointer | Yes — [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | Yes |
| Rollback points at an older version instead of rewriting bytes | Yes | Yes |
| Deploy authority | Cloudflare global rollout / placement / traffic-splitting | Local SQLite and one supervised runtime generation |
| Gradual deployments / version affinity / Cloudflare preview URLs / Workers Builds CI | Yes | Not provided |
| `oc run` origin | N/A | Loopback HTTP (or an explicitly configured local origin) |
| `oc deploy` | Wrangler deploy | HTTPS only; URLs with credentials are rejected |

