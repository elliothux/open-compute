import type { Env } from "../env";
import { ok } from "../lib/json";

export function handleHttpPing(request: Request, env: Env): Response {
  const started = Date.now();
  const url = new URL(request.url);
  return ok({
    stack: "http",
    pong: true,
    revision: env.REVISION,
    method: request.method,
    echo: url.searchParams.get("echo") ?? null,
    latencyProbeMs: Date.now() - started,
    timestamp: started,
  });
}
