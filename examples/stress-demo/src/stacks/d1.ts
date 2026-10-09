import type { Env } from "../env";
import { withD1Retry } from "../lib/d1-retry";
import { jsonError, ok, readJson } from "../lib/json";
import { ensureOrderSchema } from "../lib/schema";

interface OrderBody {
  orderId?: string;
  status?: string;
  payloadBytes?: number;
  statusFilter?: string;
  limit?: number;
}

export async function handleD1Orders(
  request: Request,
  env: Env,
): Promise<Response> {
  await ensureOrderSchema(env.DB);

  if (request.method === "POST") {
    const body = await readJson<OrderBody>(request);
    const orderId = body.orderId ?? crypto.randomUUID();
    const status = body.status ?? "created";
    const payloadBytes = body.payloadBytes ?? 0;
    const now = Date.now();

    const statements = [
      env.DB.prepare(
        "INSERT INTO orders (id, status, revision, payload_bytes, updated_at) VALUES (?1, ?2, ?3, ?4, ?5)",
      ).bind(orderId, status, env.REVISION, payloadBytes, now),
      env.DB.prepare(
        "INSERT INTO inventory_events (object_id, delta, revision, created_at) VALUES (?1, ?2, ?3, ?4)",
      ).bind(orderId, 1, env.REVISION, now),
    ];

    await withD1Retry(() => env.DB.batch(statements));

    const row = await withD1Retry(() =>
      env.DB.prepare(
        "SELECT id, status, revision, payload_bytes, updated_at FROM orders WHERE id = ?1",
      )
        .bind(orderId)
        .first(),
    );

    return ok({ stack: "d1", orderId, row, transactional: true }, 201);
  }

  if (request.method === "GET") {
    const url = new URL(request.url);
    const statusFilter = url.searchParams.get("status") ?? "created";
    const limit = Number(url.searchParams.get("limit") ?? "10");
    const rows = await withD1Retry(() =>
      env.DB.prepare(
        "SELECT id, status, revision, payload_bytes, updated_at FROM orders WHERE status = ?1 ORDER BY updated_at DESC LIMIT ?2",
      )
        .bind(statusFilter, limit)
        .all(),
    );
    return ok({
      stack: "d1",
      statusFilter,
      count: rows.results.length,
      rows: rows.results,
    });
  }

  return jsonError("method_not_allowed", 405);
}
