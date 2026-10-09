import type { Env } from "../env";
import { withD1Retry } from "../lib/d1-retry";
import { ok, structuredError } from "../lib/json";
import { ensureOrderSchema } from "../lib/schema";

function orderKey(id: string): string {
  return `order:${id}`;
}

function receiptKey(id: string): string {
  return `receipt:${id}`;
}

export async function handleScenarioVerify(
  request: Request,
  env: Env,
): Promise<Response> {
  const url = new URL(request.url);
  const orderId = url.searchParams.get("order_id");
  if (!orderId) {
    return structuredError(
      "MISSING_ORDER_ID",
      "scenario",
      { param: "order_id" },
      400,
    );
  }

  await ensureOrderSchema(env.DB);

  const kvRaw = await env.KV.get(orderKey(orderId));
  let kvStatus: string | null = null;
  if (kvRaw) {
    try {
      const parsed = JSON.parse(kvRaw) as { status?: string };
      kvStatus = parsed.status ?? kvRaw;
    } catch {
      kvStatus = kvRaw;
    }
  }

  const idempotencyRows = await withD1Retry(() =>
    env.DB.prepare(
      "SELECT key, order_id, response_json FROM idempotency WHERE order_id = ?1 ORDER BY created_at DESC LIMIT 1",
    )
      .bind(orderId)
      .all<{ key: string; order_id: string; response_json: string }>(),
  );

  const row = await withD1Retry(() =>
    env.DB.prepare(
      "SELECT id, status, revision, payload_bytes, updated_at FROM orders WHERE id = ?1",
    )
      .bind(orderId)
      .first<{
        id: string;
        status: string;
        revision: string;
        payload_bytes: number;
        updated_at: number;
      }>(),
  );

  const receipt = await env.BUCKET.get(receiptKey(orderId));
  const r2Exists = receipt !== null;

  let objectId = `inventory-${orderId.slice(0, 8)}`;
  let queueLabels: string[] = [];
  const idemRow = idempotencyRows.results[0];
  if (idemRow) {
    try {
      const cached = JSON.parse(idemRow.response_json) as {
        inventory?: { objectId?: string };
        queue?: { labels?: string[] };
      };
      objectId = cached.inventory?.objectId ?? objectId;
      queueLabels = cached.queue?.labels ?? [];
    } catch {
      // ignore malformed cache
    }
  }

  const inventory = env.INVENTORY.getByName(objectId);
  const inventoryState = await inventory.read();

  const queueChecks = [];
  for (const label of queueLabels.slice(0, 5)) {
    const processed = await env.KV.get(`queue-processed:${label}`);
    queueChecks.push({
      label,
      processed: processed !== null,
      phase: processed,
    });
  }

  const kvOk = kvStatus === "committed";
  const d1Ok = row?.status === "committed";
  const r2Ok = r2Exists;
  const consistent = kvOk && d1Ok && r2Ok;
  const queueOk =
    queueLabels.length === 0 ||
    queueChecks.every((entry) => entry.processed || entry.phase !== null);

  return ok({
    stack: "scenario",
    order_id: orderId,
    kv: { status: kvStatus, raw: kvRaw !== null },
    d1: { row },
    r2: { exists: r2Exists, key: receiptKey(orderId) },
    do: { objectId, count: inventoryState.count },
    queue: {
      labels: queueLabels,
      checks: queueChecks,
      depth: queueLabels.length,
    },
    idempotency: idemRow
      ? { key: idemRow.key, order_id: idemRow.order_id }
      : null,
    consistent,
    bindings_aligned: consistent && queueOk,
  });
}
