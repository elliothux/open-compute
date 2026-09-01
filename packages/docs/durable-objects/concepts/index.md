# 概念

所有 Durable Object 跑在本地这一个 workerd 里。`idFromName` / `newUniqueId` 仍然生成稳定 ID；`get(id, { locationHint: "wnam" })` 会接受合法 hint，但不会把对象调度到另一个地区——这里没有第二个 colo。

`jurisdiction("eu")` 同样只编码进 ID，没有地理隔离效果（`OC-DO-001`）。

每个对象有自己的 storage（KV + SQL）。SQL 看不到平台内部 alarm 表。

## Alarms

见 [Alarms](/durable-objects/alarms)。7 个成员为 `supported`，没有单独的 alarm 偏差 ID。调度仍落在这一台机器。

## WebSocket hibernation

`state.acceptWebSocket`、`webSocketMessage` / `webSocketClose` / `webSocketError`、tags、attachment serialize/deserialize 为 `supported`（19 个成员）。运行时细节见 [WebSockets](/workers/runtime-apis/websockets)。对象仍然在这一个 workerd 上，没有跨边缘休眠迁移。

## 与 Cloudflare 相同

[Durable Objects API](https://developers.cloudflare.com/durable-objects/api/) 的 namespace / stub / storage / RPC / hibernation / alarms。

## 故意不同

**`OC-DO-001`**：没有地理调度。location hint 与 jurisdiction 不改变放置。
