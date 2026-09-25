---
title: "Limits"
---

Operator-configurable product capacity limits come from the **running** binary: `limits` on `ocd capabilities --json`. Those are frozen product-specific numeric ceilings from config. **No secrets.** Worker Standard request/isolate limits are fixed separately below.

```sh
ocd --config /var/lib/open-compute/instances/default/compute.toml capabilities --json
```

Without `--config`, `limits` come from the embedded default config.

## Worker Standard resource limits

The pinned open-compute `workerd` fork enforces these request/isolate limits natively:

| Limit                                             |                                        Standard value |
| ------------------------------------------------- | ----------------------------------------------------: |
| CPU per invocation                                | 30,000 ms by default; configurable through 300,000 ms |
| Subrequests per invocation                        |    10,000 by default; configurable through 10,000,000 |
| Isolate memory                                    |                                               128 MiB |
| Startup CPU                                       |                                              1,000 ms |
| Outbound connections waiting for response headers |                                      6 per invocation |

Set the configurable dimensions with the normal Wrangler schema:

```jsonc
{
  "limits": {
    "cpu_ms": 60000,
    "subrequests": 20000,
  },
}
```

Omitted dimensions use the Standard defaults. Invalid, unknown, camelCase, zero, negative, fractional, and over-limit values are rejected. Script Settings `GET` returns effective limits; multipart `PATCH` creates a new immutable Version and may update either dimension independently.

CPU and memory termination use Cloudflare error `1102`; an uncaught subrequest-limit exception uses `1101`. Startup-limit upload validation uses `10021`. See [Workers platform limits](https://developers.cloudflare.com/workers/platform/limits/) and [Behavior differences](/docs/platform/deviations/) for the remaining hosted-only differences.

## Cache capacity

The default is 16 MiB per cached object and 1 GiB of logical body bytes per Worker, not Cloudflare's larger product quota. Exact live values still come from current `capabilities.limits`.
