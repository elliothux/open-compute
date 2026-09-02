# 概念

所有 Durable Object 运行在本机单个 workerd 进程中。`idFromName` / `newUniqueId` 仍然生成稳定 ID。`get(id, { locationHint: "wnam" })` 会接受合法 hint，但不会把对象调度到另一个地区——不提供第二个 colo。

`jurisdiction("eu")` 同样只编码进 ID，没有地理隔离效果。

每个对象有自己的 storage（KV + SQL）。SQL 看不到平台内部 alarm 表。

## Alarms

见 [Alarms](/zh/durable-objects/alarms)。`getAlarm` / `setAlarm` / `deleteAlarm` 与 `alarm()` 处理函数均支持。调度仍在本机。

## WebSocket hibernation

`state.acceptWebSocket`、`webSocketMessage` / `webSocketClose` / `webSocketError`、tags、attachment serialize/deserialize 均支持。运行时细节见 [WebSockets](/zh/workers/runtime-apis/websockets)。对象仍位于本机单个 workerd 进程，不提供跨边缘休眠迁移。

[Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) 的 namespace / stub / storage / RPC / hibernation / alarms 与 Cloudflare 对齐。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| API | [Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) | 相同：namespace / stub / storage / RPC / hibernation / alarms |
| 放置 | 地理调度 | 本机单个 workerd 进程；`locationHint` 与 jurisdiction 不改变放置 |
