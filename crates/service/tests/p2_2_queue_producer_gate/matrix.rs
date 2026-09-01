//! Worker source and persistence assertions for the P2.2 Queue matrix.

use rusqlite::Connection;
use std::path::Path;

pub(super) fn matrix_source() -> &'static str {
    r#"import { WorkerEntrypoint } from "cloudflare:workers";

const codeOf = (error) => String(error && (error.stableCode || error.message) || error);
const rejects = async (fn, code) => {
  try { await fn(); return false; } catch (error) { return codeOf(error).includes(code); }
};

export class Named extends WorkerEntrypoint {
  async fetch() {
    const result = await this.env.EVENTS.send("named", { contentType: "text", delaySeconds: 0 });
    return new Response(`named:${result.metadata.metrics.backlogCount}:${result.metadata.metrics.oldestMessageTimestamp instanceof Date}`);
  }
}

export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    if (path === "/metrics") return Response.json(await env.EVENTS.metrics());
    if (path === "/send-one") {
      try { return Response.json(await env.EVENTS.send({ after: "reconcile" })); }
      catch (error) { return new Response(codeOf(error), { status: 500 }); }
    }
    if (path !== "/matrix") return new Response("plain");
    const initial = await env.EVENTS.metrics();
    await env.EVENTS.send({ marker: "matrix-json-body" });
    await env.EVENTS.send("héllo", { contentType: "text", delaySeconds: 0 });
    const bytes = new Uint8Array([1, 2, 3]);
    const pending = env.EVENTS.send(bytes, { contentType: "bytes", delaySeconds: 1 });
    const bytesDetached = bytes.buffer.detached === true;
    await pending;
    const cycle = { v8: true, when: new Date(1_700_000_000_000) };
    cycle.self = cycle;
    cycle.items = new Map([["k", new Set([1, 2])]]);
    const v8 = await env.EVENTS.send(cycle, { contentType: "v8", delaySeconds: 0 });
    function* messages() {
      yield { body: "batch-a", contentType: "text" };
      yield { body: { batch: "b" }, delaySeconds: 0 };
      yield { body: new Uint8Array([4, 5]), contentType: "bytes", delaySeconds: 9 };
    }
    const response = await env.EVENTS.sendBatch(messages(), { delaySeconds: 7 });
    const failures = [
      await rejects(() => env.EVENTS.send("x", { contentType: "xml" }), "QUEUE_CONTENT_TYPE_UNSUPPORTED"),
      await rejects(() => env.EVENTS.send(new Uint8Array(128001), { contentType: "bytes" }), "QUEUE_MESSAGE_TOO_LARGE"),
      await rejects(() => env.EVENTS.send("x", { contentType: "text", delaySeconds: 86401 }), "QUEUE_DELAY_INVALID"),
      await rejects(() => env.EVENTS.sendBatch([]), "QUEUE_INVALID_MESSAGE"),
      await rejects(() => env.EVENTS.sendBatch(Array.from({ length: 101 }, () => ({ body: 1 }))), "QUEUE_BATCH_LIMIT_EXCEEDED"),
      await rejects(() => env.EVENTS.send(undefined), "QUEUE_INVALID_MESSAGE"),
      await rejects(() => env.EVENTS.send("x", { unexpected: true }), "QUEUE_INVALID_MESSAGE"),
    ];
    return Response.json({
      initialCount: initial.backlogCount,
      initialOldestUndefined: initial.oldestMessageTimestamp === undefined,
      backlogCount: response.metadata.metrics.backlogCount,
      oldestIsDate: response.metadata.metrics.oldestMessageTimestamp instanceof Date,
      bytesDetached,
      v8RoundTrip: v8.metadata.metrics.backlogCount === 4,
      errors: failures.filter(Boolean).length,
    });
  },
  async queue(batch, env, ctx) {
    ctx.waitUntil(Promise.resolve());
    if (typeof batch.queue !== "string" || !batch.metadata || !batch.metadata.metrics) {
      throw new Error("missing batch metadata");
    }
    const metrics = batch.metadata.metrics;
    if (typeof metrics.backlogCount !== "number" || typeof metrics.backlogBytes !== "number") {
      throw new Error("invalid metrics");
    }
    if (metrics.oldestMessageTimestamp !== undefined
        && !(metrics.oldestMessageTimestamp instanceof Date)) {
      throw new Error("oldest timestamp");
    }
    for (const message of batch.messages) {
      if (!(message.timestamp instanceof Date) || message.attempts < 1) throw new Error("message");
      if (message.body === "throw") throw new Error("handler throw");
      if (message.body === "retry-me") {
        message.retry({ delaySeconds: 4 });
        message.ack();
        continue;
      }
      if (message.body && message.body.v8 === true) {
        if (!(message.body.when instanceof Date) || !(message.body.items instanceof Map)
            || !(message.body.items.get("k") instanceof Set) || message.body.self !== message.body) {
          throw new Error("v8 body");
        }
      }
      message.ack();
    }
    batch.ackAll();
  }
};"#
}

pub(super) fn assert_persisted_frames(path: &Path) {
    let connection = Connection::open(path).unwrap();
    let mut statement = connection
        .prepare(
            "SELECT content_type, body, available_at_ms - enqueued_at_ms
             FROM queue_messages ORDER BY seq",
        )
        .unwrap();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 8);
    assert_eq!(
        rows[0],
        (
            "json".to_owned(),
            br#"{"marker":"matrix-json-body"}"#.to_vec(),
            5_000
        )
    );
    assert_eq!(rows[1], ("text".to_owned(), "héllo".as_bytes().to_vec(), 0));
    assert_eq!(rows[2], ("bytes".to_owned(), vec![1, 2, 3], 1_000));
    assert_eq!(rows[3].0, "v8");
    assert!(rows[3].1.starts_with(&[0x4f, 0x43, 0x44, 0x56]));
    assert_eq!(rows[3].2, 0);
    assert_eq!(rows[4].2, 7_000);
    assert_eq!(rows[5].2, 0);
    assert_eq!(rows[6].2, 9_000);
    assert_eq!(rows[7], ("text".to_owned(), b"named".to_vec(), 0));
}

pub(super) fn persisted_v8_body(path: &Path) -> Vec<u8> {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT body FROM queue_messages WHERE content_type = 'v8' ORDER BY seq LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

pub(super) fn max_expiry(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT MAX(expires_at_ms) FROM queue_messages", [], |row| {
            row.get(0)
        })
        .unwrap()
}
