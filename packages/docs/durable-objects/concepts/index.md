# Concepts

Every Durable Object runs in the one local workerd. `idFromName` / `newUniqueId` still mint stable IDs. `get(id, { locationHint: "wnam" })` accepts a legal hint but does not schedule the object into another region — a second colo is not provided.

`jurisdiction("eu")` is likewise encoded into the ID with no geographic isolation effect.

Each object has its own storage (KV + SQL). SQL cannot see the platform-internal alarm table.

## Alarms

See [Alarms](/durable-objects/alarms). `getAlarm` / `setAlarm` / `deleteAlarm` and the `alarm()` handler are supported. Scheduling still happens on this node.

## WebSocket hibernation

`state.acceptWebSocket`, `webSocketMessage` / `webSocketClose` / `webSocketError`, tags, and attachment serialize/deserialize are supported. Runtime details: [WebSockets](/workers/runtime-apis/websockets). The object still lives on this one workerd. Cross-edge hibernation migration is not provided.

The [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) for namespace / stub / storage / RPC / hibernation / alarms matches Cloudflare.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | Same: namespace / stub / storage / RPC / hibernation / alarms |
| Placement | Geographic scheduling | One local workerd; location hints and jurisdiction do not change placement |
