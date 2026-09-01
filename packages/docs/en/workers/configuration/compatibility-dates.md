# Compatibility dates

This platform has one effective compatibility date: `effective_compatibility_date` from the formal runtime lock, currently **`2026-08-30`**. Projects cannot choose a date.

```sh
ocd capabilities --json
```

Read `runtime.effective_compatibility_date`. Do not copy this page as the live value; trust the JSON on this machine.

## Same as Cloudflare

The **meaning** of the date matches Cloudflare: it selects workerd's observable behavior as of that day. See [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/).

## Intentional delta

There is no per-project `compatibility_date` / `compatibilityDate`. Putting one in `open-compute.json` is an unknown field and fails. Changing the date means changing the platform pin (`workerd.lock.json`), not a developer config knob.
