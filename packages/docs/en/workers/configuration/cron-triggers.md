# Cron Triggers

The Cron product lives here: the Worker's `scheduled()` runs on UTC expressions. This is not the Cloudflare dashboard trigger UI.

```ts
export default {
  async scheduled(controller: ScheduledController, env: Env, ctx: ExecutionContext): Promise<void> {
    if (shouldGiveUp()) controller.noRetry();
  },
} satisfies ExportedHandler<Env>;
```

`controller.cron` is the exact string declared on the deployment. `controller.scheduledTime` is that logical slot's time.

## Expressions

Only **five UTC fields**: minute, hour, day-of-month, month, day-of-week. No seconds field, no year, no local timezone or DST.

Documented local Quartz-like extensions: `*` `,` `-` `/` `L` `W` `#`, plus case-insensitive three-letter month/weekday names. Weekday numbers follow the Cloudflare fixture: `1=Sunday` … `7=Saturday`.

The platform deployment metadata field is `crons: string[]`. `open-compute.json` has no Wrangler `triggers` / `triggers.crons`; adding one is an unknown field and fails. Workflow cron uses `schedules` on the workflow binding, not Worker `scheduled()`.

## Same as Cloudflare

The handler matches [scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/). Five-field cron matches the common surface of [Cloudflare Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/). `noRetry()` is available.

## Intentional delta: OC-CRON-001

Cron is UTC-only with five fields and the documented local Quartz-like extensions. Recovery projects at most the newest slot within the configured misfire grace rather than replaying complete downtime history. Known failures use the configured bounded local retry policy unless `noRetry()` is called.

Default `scheduler.cron_misfire_grace_ms = 300000` (five minutes). Exact values come from `ocd capabilities --json` `limits`. Recovering from five hours of downtime does not replay every minute of that window.
