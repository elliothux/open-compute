# Changelog

Trust the release identity on the node, not prose in this file.

```sh
ocd capabilities --json
```

Read `release`: `platform_version`, `git_revision`, `workerd_version`, `workerd_lock_sha256`, schema versions. Repo git tags (if present) point at source; **the running contract is `capabilities.release`.**

This slot matches [Cloudflare Workers changelog](https://developers.cloudflare.com/workers/platform/changelog/). That is hosted release notes, not this binary's.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Hand-written date list | Yes | Not provided |
| workerd / types pin change | Hosted release | A dependency bump; shows up as `effective_compatibility_date` and `workerd_version` in the lock |
| Current lock date | N/A | `2026-08-30`; if the JSON differs, trust the JSON |

