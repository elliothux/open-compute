import type { Env } from "../env";
import { withD1Retry } from "../lib/d1-retry";
import { ok, readJson, structuredError, type StackName } from "../lib/json";
import { ensureOrderSchema } from "../lib/schema";

type ScenarioMode = "normal" | "peak" | "boundary" | "fault";
type FaultStack = StackName | "outbound";

interface MegaCheckoutBody {
  orderId?: string;
  idempotencyKey?: string;
  mode?: ScenarioMode;
  faultStack?: FaultStack;
  fanOutN?: number;
  fanOutM?: number;
  payloadBytes?: number;
  objectId?: string;
  hotKey?: boolean;
  forceD1Conflict?: boolean;
}

interface MegaCheckoutResult {
  orderId: string;
  idempotencyKey: string;
  mode: ScenarioMode;
  status: string;
  duplicate: boolean;
  revision: string;
  kv: { orderKey: string; idempotencyKey: string };
  d1: { row: unknown; conflictInjected: boolean };
  r2: { key: string; bytes: number };
  inventory: { objectId: string; count: number };
  queue: { labels: string[]; enqueued: number };
  workflow: { workflowId: string };
  fetch: { ok: boolean; status: number };
  verify: Record<string, unknown>;
}

function orderKey(id: string): string {
  return `order:${id}`;
}

function receiptKey(id: string): string {
  return `receipt:${id}`;
}

async function readIdempotent(
  env: Env,
  key: string,
): Promise<MegaCheckoutResult | null> {
  const cached = await env.KV.get(key);
  if (!cached) {
    return null;
  }
  return JSON.parse(cached) as MegaCheckoutResult;
}

async function storeIdempotent(
  env: Env,
  key: string,
  result: MegaCheckoutResult,
): Promise<void> {
  await env.KV.put(key, JSON.stringify(result));
  await withD1Retry(() =>
    env.DB.prepare(
      "INSERT INTO idempotency (key, order_id, response_json, created_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(key) DO UPDATE SET response_json = excluded.response_json, created_at = excluded.created_at",
    )
      .bind(key, result.orderId, JSON.stringify(result), Date.now())
      .run(),
  );
}

async function rollbackPartial(
  env: Env,
  orderId: string,
  objectId: string,
): Promise<void> {
  await env.KV.delete(orderKey(orderId));
  await withD1Retry(() =>
    env.DB.prepare("DELETE FROM orders WHERE id = ?1").bind(orderId).run(),
  );
  await env.BUCKET.delete(receiptKey(orderId));
  const inventory = env.INVENTORY.getByName(objectId);
  await inventory.increment(-1).catch(() => undefined);
}

export async function handleMegaCheckout(
  request: Request,
  env: Env,
): Promise<Response> {
  if (request.method !== "POST") {
    return structuredError("METHOD_NOT_ALLOWED", "scenario", {}, 405);
  }

  await ensureOrderSchema(env.DB);
  const body = await readJson<MegaCheckoutBody>(request);
  const orderId = body.orderId ?? crypto.randomUUID();
  const idempotencyKey = body.idempotencyKey ?? `idem-${orderId}`;
  const mode = body.mode ?? "normal";
  const faultStack = body.faultStack;
  const fanOutN = body.fanOutN ?? (mode === "peak" ? 10 : 3);
  const fanOutM = body.fanOutM ?? 3;
  const payloadBytes =
    body.payloadBytes ?? (mode === "boundary" ? 65536 : 1024);
  const objectId = body.hotKey
    ? "hot-inventory"
    : (body.objectId ?? `inventory-${orderId.slice(0, 8)}`);
  const forceD1Conflict = body.forceD1Conflict === true;

  if (mode === "fault" && faultStack === "kv") {
    return structuredError("KV_FAULT_INJECTED", "kv", { orderId, mode }, 503);
  }

  const existing = await readIdempotent(env, idempotencyKey);
  if (existing) {
    return ok({ ...existing, duplicate: true, status: "replayed" });
  }

  const now = Date.now();
  let conflictInjected = false;

  if (mode === "fault" && faultStack === "d1") {
    return structuredError("D1_FAULT_INJECTED", "d1", { orderId, mode }, 503);
  }

  if (forceD1Conflict) {
    await withD1Retry(() =>
      env.DB.prepare(
        "INSERT INTO orders (id, status, revision, payload_bytes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
      )
        .bind(orderId, "pending", env.REVISION, payloadBytes, now)
        .run(),
    ).catch(() => {
      conflictInjected = true;
    });
  }

  await env.KV.put(
    orderKey(orderId),
    JSON.stringify({ status: "pending", revision: env.REVISION, now }),
  );

  if (mode === "fault" && faultStack === "r2") {
    await env.KV.delete(orderKey(orderId));
    return structuredError(
      "R2_FAULT_INJECTED",
      "r2",
      { orderId, mode, rolled_back: ["kv"] },
      503,
    );
  }

  await withD1Retry(() =>
    env.DB.batch([
      env.DB.prepare(
        "INSERT INTO orders (id, status, revision, payload_bytes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(id) DO UPDATE SET status = excluded.status, revision = excluded.revision, payload_bytes = excluded.payload_bytes, updated_at = excluded.updated_at",
      ).bind(orderId, "pending", env.REVISION, payloadBytes, now),
      env.DB.prepare(
        "INSERT INTO inventory_events (object_id, delta, revision, created_at) VALUES (?1, ?2, ?3, ?4)",
      ).bind(objectId, fanOutM, env.REVISION, now),
    ]),
  );

  if (mode === "fault" && faultStack === "queue") {
    await rollbackPartial(env, orderId, objectId);
    return structuredError(
      "QUEUE_FAULT_INJECTED",
      "queue",
      { orderId, mode, rolled_back: ["kv", "d1"] },
      503,
    );
  }

  const receiptPayload = JSON.stringify({
    orderId,
    revision: env.REVISION,
    payloadBytes,
    fanOutN,
    fanOutM,
    checksum: payloadBytes % 997,
  });
  await env.BUCKET.put(receiptKey(orderId), receiptPayload);

  if (mode === "fault" && faultStack === "do") {
    await rollbackPartial(env, orderId, objectId);
    return structuredError(
      "DO_FAULT_INJECTED",
      "do",
      { orderId, mode, rolled_back: ["kv", "d1", "r2"] },
      503,
    );
  }

  const inventory = env.INVENTORY.getByName(objectId);
  const inventoryResult = await inventory.reserve(fanOutM);

  const queueLabels: string[] = [];
  const batch = [];
  for (let index = 0; index < fanOutN; index += 1) {
    const label = `mega:${orderId}:${index}`;
    queueLabels.push(label);
    batch.push({
      body: {
        label,
        orderId,
        phase: "mega-checkout",
        revision: env.REVISION,
        index,
      },
    });
  }
  await env.EVENTS.sendBatch(batch, { delaySeconds: 0 });

  if (mode === "fault" && faultStack === "workflow") {
    return structuredError(
      "WORKFLOW_FAULT_INJECTED",
      "workflow",
      { orderId, mode, counter: inventoryResult.count },
      503,
    );
  }

  const workflow = await env.FLOW.create({
    id: `mega-${orderId}`,
    params: { orderId, mode, fanOutN: fanOutM },
  });

  if (mode === "fault" && faultStack === "fetch") {
    return structuredError(
      "FETCH_FAULT_INJECTED",
      "fetch",
      { orderId, mode, workflowId: workflow.id },
      503,
    );
  }

  let fetchOk = false;
  let fetchStatus = 0;
  if (mode !== "fault") {
    const outbound = await fetch(env.OUTBOUND_URL);
    fetchOk = outbound.ok;
    fetchStatus = outbound.status;
  }

  if (mode === "fault" && faultStack === "outbound") {
    return structuredError(
      "OUTBOUND_FAULT_INJECTED",
      "fetch",
      { orderId, mode, workflowId: workflow.id },
      503,
    );
  }

  await env.KV.put(
    orderKey(orderId),
    JSON.stringify({
      status: "committed",
      revision: env.REVISION,
      now: Date.now(),
    }),
  );
  await withD1Retry(() =>
    env.DB.prepare(
      "UPDATE orders SET status = ?1, updated_at = ?2 WHERE id = ?3",
    )
      .bind("committed", Date.now(), orderId)
      .run(),
  );

  const row = await withD1Retry(() =>
    env.DB.prepare(
      "SELECT id, status, revision, payload_bytes, updated_at FROM orders WHERE id = ?1",
    )
      .bind(orderId)
      .first(),
  );

  const result: MegaCheckoutResult = {
    orderId,
    idempotencyKey,
    mode,
    status: "committed",
    duplicate: false,
    revision: env.REVISION,
    kv: { orderKey: orderKey(orderId), idempotencyKey },
    d1: { row, conflictInjected },
    r2: { key: receiptKey(orderId), bytes: receiptPayload.length },
    inventory: { objectId, count: inventoryResult.count },
    queue: { labels: queueLabels, enqueued: queueLabels.length },
    workflow: { workflowId: workflow.id },
    fetch: { ok: fetchOk, status: fetchStatus },
    verify: {
      idempotencyKey,
      queueLabels,
      objectId,
      workflowId: workflow.id,
    },
  };

  await storeIdempotent(env, idempotencyKey, result);
  return ok(result as unknown as Record<string, unknown>);
}
