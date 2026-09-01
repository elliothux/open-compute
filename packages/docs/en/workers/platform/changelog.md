# Changelog

Do not invent dates or a version narrative on this page. Trust the release identity on this machine.

```sh
ocd capabilities --json
```

Read `release`: `platform_version`, `git_revision`, `workerd_version`, `workerd_lock_sha256`, schema versions. Repo git tags (if present) point at source; **the running contract is `capabilities.release`, not prose in this file.**

## Same as Cloudflare

This slot matches [Cloudflare Workers changelog](https://developers.cloudflare.com/workers/platform/changelog/). That is their hosted release notes, not this binary's.

## Intentional delta

open-compute does not keep a hand-written date list. A workerd / types pin change is a dependency bump and shows up as `effective_compatibility_date` and `workerd_version` in the lock. The current lock date is `2026-08-30`; if the JSON differs, trust the JSON.
