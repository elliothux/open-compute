# Cron Triggers

Cron on this platform runs the Worker's `scheduled()` on UTC expressions. This is not the Cloudflare dashboard trigger UI.

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

Declare Worker cron triggers with standard `triggers.crons`. Workflow schedules remain on the standard Workflow binding and do not invoke the Worker's `scheduled()` handler.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| `scheduled()` handler | Yes — [scheduled()](https://developers.cloudflare.com/workers/runtime-apis/handlers/scheduled/) | Yes |
| Five-field cron | Yes — [Cloudflare Cron Triggers](https://developers.cloudflare.com/workers/configuration/cron-triggers/) | Yes; UTC only |
| `noRetry()` | Yes | Yes |
| `triggers.crons` in the project file | Wrangler | Not allowed; deployment metadata field is `crons: string[]` |
| Misfire recovery | Hosted scheduler semantics | Projects at most the latest slot within grace; does not replay complete downtime history |
| Retry on known failure | Hosted policy | Configured bounded local retry unless `noRetry()` is called |
| Default misfire grace | Plan-dependent | `scheduler.cron_misfire_grace_ms = 300000` (five minutes); exact values from `ocd capabilities --json` `limits` |
