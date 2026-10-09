import type { Env } from "../env";
import { jsonError, ok, readJson } from "../lib/json";

interface IncrementBody {
  amount?: number;
}

async function handleDoRead(env: Env, objectId: string): Promise<Response> {
  const stub = env.INVENTORY.getByName(objectId);
  const state = await stub.read();
  return ok({ stack: "do", ...state });
}

export async function handleDoIncrement(
  request: Request,
  env: Env,
  objectId: string,
): Promise<Response> {
  if (request.method === "GET") {
    return handleDoRead(env, objectId);
  }
  if (request.method !== "POST") {
    return jsonError("method_not_allowed", 405);
  }
  const body = await readJson<IncrementBody>(request);
  const amount = body.amount ?? 1;
  const stub = env.INVENTORY.getByName(objectId);
  const result = await stub.increment(amount);
  const state = await stub.read();
  return ok({
    stack: "do",
    objectId,
    amount,
    count: result.count,
    alarmTicks: state.alarmTicks,
    lastAlarmAt: state.lastAlarmAt,
  });
}

export async function handleDoWebSocket(
  request: Request,
  env: Env,
  objectId: string,
): Promise<Response> {
  if (request.headers.get("Upgrade")?.toLowerCase() !== "websocket") {
    return jsonError("expected_websocket", 426);
  }
  const stub = env.INVENTORY.getByName(objectId);
  return stub.fetch(request);
}
