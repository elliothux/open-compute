---
title: "Secrets"
---

Manage Worker secrets with the exact project-local Wrangler. Secret values are read by Wrangler from stdin and must not appear in `wrangler.jsonc`, package scripts, command arguments, target records, or logs.

```sh
ocd wrangler --target staging secret put API_TOKEN --env staging
ocd wrangler --target staging secret list --env staging
ocd wrangler --target staging secret delete API_TOKEN --env staging
ocd wrangler --target staging secret bulk ./secrets.json --env staging
```

Use a deployer target. The target's deployer token authorizes the management request but is not exposed to the Worker. `ocd` reads that credential from its owner-only external token file and places it only in the short-lived Wrangler child environment.

Secret mutation follows the immutable Version model: open-compute encrypts the value and creates a new Version and 100% Deployment where required. List and get responses expose names and types only, never plaintext. Rollback changes the active Version pointer and therefore restores that Version's secret bindings without rewriting it.

Cloudflare Secrets Store and Dashboard secret management are not provided. See [Develop](/docs/develop/) for target setup, CI handling, and failure recovery.
