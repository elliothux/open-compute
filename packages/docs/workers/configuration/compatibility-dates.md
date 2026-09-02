# Compatibility dates

This platform has one effective compatibility date: `effective_compatibility_date` from the formal runtime lock, currently **`2026-08-30`**. Projects cannot choose a date.

```sh
ocd capabilities --json
```

Read `runtime.effective_compatibility_date`. The live value is the JSON on the node.

The meaning of the date matches Cloudflare: it selects workerd's observable behavior as of that day. See [Cloudflare compatibility dates](https://developers.cloudflare.com/workers/configuration/compatibility-dates/).

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| Date selects workerd observable behavior | Yes | Yes |
| Per-project `compatibility_date` / `compatibilityDate` | Yes | Not allowed; putting one in `open-compute.json` is an unknown field and fails |
| How the date changes | Project config | Changing the platform pin (`workerd.lock.json`) |
