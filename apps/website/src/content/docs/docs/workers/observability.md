---
title: "Logs and live tail"
description: "Persist, query, and tail Worker logs on the selected open-compute instance."
---

open-compute implements the Cloudflare Workers observability settings, telemetry query routes, and selected-script live tail on the local instance. `ocd wrangler tail` uses the same Cloudflare-compatible `/client/v4` surface.

```sh
ocd wrangler tail --env staging
```

## Configure logs

Use Wrangler's `observability` configuration. Log enablement, sampling, invocation logs, and persistence are supported. External log destinations and traces are not.

```json
{
  "observability": {
    "enabled": true,
    "head_sampling_rate": 1,
    "logs": { "enabled": true, "invocation_logs": true, "persist": true },
    "traces": { "enabled": false }
  }
}
```

Persisted telemetry supports the `events` and `invocations` query views, keys and values discovery, filters, and one live-tail session scoped to a selected script. Retention, database size, invocation log size, query timeframe and event count, ingest queue capacity, tail session count, and tail client buffering are bounded by the instance configuration.

## Local differences

Telemetry is stored on the selected instance rather than Cloudflare's global analytics service. Hosted-only metadata is omitted, regex filters use a bounded RE2-compatible subset, and account-wide live tail is unavailable. Calculations, traces, agents, requests, saved queries, Tail Workers, Logpush, and external destinations are not supported.

See [Behavior differences](/docs/platform/deviations/), [Limits](/docs/platform/limits/), and the [Management SDK](/docs/platform/reference/sdk/).
