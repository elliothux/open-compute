import { withD1Retry } from "./d1-retry";

const schemaByDatabase = new WeakMap<D1Database, Promise<void>>();

async function initOrderSchema(db: D1Database): Promise<void> {
  await withD1Retry(() =>
    db
      .prepare(
        "CREATE TABLE IF NOT EXISTS orders (id TEXT PRIMARY KEY, status TEXT NOT NULL, revision TEXT NOT NULL, payload_bytes INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL)",
      )
      .run(),
  );

  const columns = await withD1Retry(() =>
    db.prepare("PRAGMA table_info(orders)").all<{ name: string }>(),
  );
  const names = new Set(columns.results.map((column) => column.name));
  if (!names.has("payload_bytes")) {
    await withD1Retry(() =>
      db
        .prepare(
          "ALTER TABLE orders ADD COLUMN payload_bytes INTEGER NOT NULL DEFAULT 0",
        )
        .run(),
    );
  }

  await withD1Retry(() =>
    db
      .prepare(
        "CREATE INDEX IF NOT EXISTS idx_orders_status_updated ON orders(status, updated_at DESC)",
      )
      .run(),
  );
  await withD1Retry(() =>
    db
      .prepare(
        "CREATE TABLE IF NOT EXISTS idempotency (key TEXT PRIMARY KEY, order_id TEXT NOT NULL, response_json TEXT NOT NULL, created_at INTEGER NOT NULL)",
      )
      .run(),
  );
  await withD1Retry(() =>
    db
      .prepare(
        "CREATE TABLE IF NOT EXISTS inventory_events (object_id TEXT NOT NULL, delta INTEGER NOT NULL, revision TEXT NOT NULL, created_at INTEGER NOT NULL)",
      )
      .run(),
  );
  await withD1Retry(() =>
    db
      .prepare(
        "CREATE INDEX IF NOT EXISTS idx_inventory_events_object ON inventory_events(object_id, created_at DESC)",
      )
      .run(),
  );
}

export async function ensureOrderSchema(db: D1Database): Promise<void> {
  let ready = schemaByDatabase.get(db);
  if (!ready) {
    ready = initOrderSchema(db);
    schemaByDatabase.set(db, ready);
  }
  await ready;
}
