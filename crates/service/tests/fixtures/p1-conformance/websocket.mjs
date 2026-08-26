export function checkWebSocketSurface() {
  return typeof WebSocketPair === "function" && typeof WebSocket === "function";
}
