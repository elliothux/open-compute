# Versions and deployments

One deploy: create or reuse a Worker → encode an immutable bundle → validate the runtime → activate (promote). Authority is local SQLite and one supervised runtime generation. `oc run` is this path locally; `oc deploy` is remote HTTPS.

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

A failed validation does not change the current active deployment. Promotion / rollback change the active pointer; they do not mutate a ready deployment's bytes. Preview is the local URL printed by `oc run`.

## Same as Cloudflare

Versions are immutable; a release switches the active pointer. Rollback points at an older version instead of rewriting bytes. See [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/).

## Intentional delta: OC-DEPLOY-001

Deployments, routes, promotion, and rollback use one local SQLite authority and one supervised runtime generation. The platform does not claim Cloudflare's global rollout, placement, traffic-splitting, account-management, or billing control planes.

No gradual deployments, version affinity, Cloudflare preview URLs, or Workers Builds CI. `run` requires loopback HTTP (or your explicitly configured local origin); remote `deploy` accepts HTTPS only, not URLs with credentials.
