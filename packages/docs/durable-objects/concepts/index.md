# 概念

所有 Durable Object 跑在本地这一个 workerd 里。`idFromName` / `newUniqueId` 仍然生成稳定 ID。`get(id, { locationHint: "wnam" })` 会接受合法 hint，但不会把对象调度到另一个地区——不提供第二个 colo。

`jurisdiction("eu")` 同样只编码进 ID，没有地理隔离效果。

每个对象有自己的 storage（KV + SQL）。SQL 看不到平台内部 alarm 表。

## Alarms

见 [Alarms](/durable-objects/alarms)。`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` handler 均支持。调度仍落在该节点。

## WebSocket hibernation

`state.acceptWebSocket`、`webSocketMessage` / `webSocketClose` / `webSocketError`、tags、attachment serialize/deserialize 均支持。运行时细节见 [WebSockets](/workers/runtime-apis/websockets)。对象仍然在这一个 workerd 上，不提供跨边缘休眠迁移。

[Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) 的 namespace / stub / storage / RPC / hibernation / alarms 与 Cloudflare 对齐。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | 相同：namespace / stub / storage / RPC / hibernation / alarms |
| 放置 | 地理调度 | 本地这一个 workerd；`locationHint` 与 jurisdiction 不改变放置 |
