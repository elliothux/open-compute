# Concepts

Every Durable Object runs in the one local workerd. `idFromName` / `newUniqueId` still mint stable IDs. `get(id, { locationHint: "wnam" })` accepts a legal hint but does not schedule the object into another region — there is no second colo.

`jurisdiction("eu")` is likewise encoded into the ID with no geographic isolation effect (`OC-DO-001`).

Each object has its own storage (KV + SQL). SQL cannot see the platform-internal alarm table.

## Alarms

See [Alarms](/en/durable-objects/alarms). 7 members are `supported`; there is no separate alarm deviation ID. Scheduling still happens on this machine.

## WebSocket hibernation

`state.acceptWebSocket`, `webSocketMessage` / `webSocketClose` / `webSocketError`, tags, and attachment serialize/deserialize are `supported` (19 members). Runtime details: [WebSockets](/en/workers/runtime-apis/websockets). The object still lives on this one workerd; there is no cross-edge hibernation migration.

## Same as Cloudflare

The [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) for namespace / stub / storage / RPC / hibernation / alarms.

## Intentional differences

**`OC-DO-001`**: no geographic scheduling. Location hints and jurisdiction do not change placement.
