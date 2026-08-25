import { DurableObject } from "cloudflare:workers";
import { D1Database } from "./__open_compute_d1_facade__.js";
import { DurableObjectNamespace } from "./__open_compute_do_facade__.js";
import { R2Bucket } from "./__open_compute_r2_facade__.js";

function errorCode(error) {
  return String(error && error.message ? error.message : error);
}

function scalar(sql, query, fallback = null) {
  const rows = sql.exec(query).toArray();
  return rows.length ? rows[0].value : fallback;
}

export class AppObject extends DurableObject {
  constructor(ctx, env) {
    super(ctx, env);
    this.ctx = ctx;
    this.env = env;
    this.ctx.storage.sql.exec(
      "CREATE TABLE IF NOT EXISTS state(" +
      "id INTEGER PRIMARY KEY CHECK(id = 1), count INTEGER NOT NULL, " +
      "alarm_deliveries INTEGER NOT NULL, alarm_release TEXT, alarm_retry_count INTEGER)"
    );
    this.ctx.storage.sql.exec(
      "INSERT INTO state(id, count, alarm_deliveries) VALUES(1, 0, 0) " +
      "ON CONFLICT(id) DO NOTHING"
    );
  }

  async increment() {
    this.ctx.storage.sql.exec("UPDATE state SET count = count + 1 WHERE id = 1");
    const count = Number(scalar(this.ctx.storage.sql, "SELECT count AS value FROM state WHERE id = 1", 0));
    this.ctx.storage.kv.put("sync-count", count);
    await this.ctx.storage.transaction(async (txn) => {
      await txn.put("async-count", count);
      await txn.put("release", this.env.RELEASE);
    });
    return this.snapshot();
  }

  async snapshot() {
    const row = this.ctx.storage.sql.exec(
      "SELECT count, alarm_deliveries, alarm_release, alarm_retry_count FROM state WHERE id = 1"
    ).toArray()[0];
    return {
      release: this.env.RELEASE,
      count: Number(row.count),
      syncCount: Number(this.ctx.storage.kv.get("sync-count") || 0),
      asyncCount: Number(await this.ctx.storage.get("async-count") || 0),
      alarm: await this.ctx.storage.getAlarm(),
      alarmDeliveries: Number(row.alarm_deliveries),
      alarmRelease: row.alarm_release,
      alarmRetryCount: row.alarm_retry_count === null ? null : Number(row.alarm_retry_count),
    };
  }

  async setAlarmAt(time) {
    await this.ctx.storage.setAlarm(time);
    return this.ctx.storage.getAlarm();
  }

  async alarm(info) {
    this.ctx.storage.sql.exec(
      "UPDATE state SET alarm_deliveries = alarm_deliveries + 1, " +
      "alarm_release = ?, alarm_retry_count = ? WHERE id = 1",
      this.env.RELEASE,
      info.retryCount
    );
  }

  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/fetch") {
      const value = await this.snapshot();
      return Response.json({ release: value.release, count: value.count });
    }
    if (url.pathname === "/websocket") {
      const pair = new WebSocketPair();
      const [client, server] = Object.values(pair);
      server.accept();
      server.addEventListener("message", async (event) => {
        const value = event.data instanceof Blob ? await event.data.arrayBuffer() : event.data;
        server.send(value);
      });
      return new Response(null, { status: 101, webSocket: client });
    }
    return new Response("missing", { status: 404 });
  }
}

export class OtherObject extends AppObject {}

function primaryStub(env) {
  return env.OBJECTS.getByName("primary");
}

async function snapshot(env) {
  const kvMetadata = await env.CACHE.getWithMetadata("text", "text");
  const kvMany = Array.from((await env.CACHE.get(["text", "missing"], "text")).entries());
  const kvList = await env.CACHE.list({ prefix: "", limit: 100 });
  const kvStream = await new Response(await env.CACHE.get("stream", "stream")).text();
  const kvBinary = Array.from(new Uint8Array(await env.CACHE.get("binary", "arrayBuffer")));

  const r2Head = await env.BUCKET.head("doc.txt");
  const r2Headers = new Headers();
  const metadataReturn = r2Head.writeHttpMetadata(r2Headers);
  const r2Object = await env.BUCKET.get("doc.txt");
  const r2Range = await env.BUCKET.get("doc.txt", { range: { offset: 1, length: 3 } });
  const r2List = await env.BUCKET.list({
    prefix: "doc",
    limit: 10,
    include: ["httpMetadata", "customMetadata"],
  });

  const d1All = await env.DB.prepare("SELECT id, body FROM notes ORDER BY id").all();
  const d1First = await env.DB.prepare("SELECT body FROM notes WHERE id = ?1").bind(1).first("body");
  const d1Raw = await env.DB.prepare("SELECT id, body FROM notes ORDER BY id").raw({ columnNames: true });
  const d1Session = await env.DB.withSession("first-primary")
    .prepare("SELECT count(*) AS value FROM notes").first("value");

  const stub = primaryStub(env);
  const fetched = await stub.fetch(new Request("https://object.invalid/fetch"));
  return {
    release: env.RELEASE,
    facade: {
      kv: typeof env.CACHE.get === "function"
        && typeof env.CACHE.put === "function"
        && typeof env.CACHE.list === "function",
      r2: env.BUCKET instanceof R2Bucket && typeof env.BUCKET.fetch === "undefined",
      d1: env.DB instanceof D1Database && typeof env.DB.fetch === "undefined",
      durableObject: env.OBJECTS instanceof DurableObjectNamespace,
    },
    kv: {
      text: kvMetadata.value,
      metadata: kvMetadata.metadata,
      json: await env.CACHE.get("json", "json"),
      binary: kvBinary,
      stream: kvStream,
      many: kvMany,
      keys: kvList.keys.map((item) => item.name).sort(),
      isolated: await env.CACHE_OTHER.get("text", "text"),
    },
    r2: {
      body: await r2Object.text(),
      range: await r2Range.text(),
      size: r2Head.size,
      custom: r2Head.customMetadata.stage,
      contentType: r2Headers.get("content-type"),
      metadataReturnUndefined: metadataReturn === undefined,
      listed: r2List.objects.map((item) => item.key),
      isolated: await (await env.BUCKET_OTHER.get("doc.txt")).text(),
    },
    d1: {
      rows: d1All.results,
      first: d1First,
      raw: d1Raw,
      sessionCount: d1Session,
      isolated: await env.DB_OTHER.prepare("SELECT body FROM notes WHERE id = 1").first("body"),
    },
    durableObject: {
      rpc: await stub.snapshot(),
      fetch: await fetched.json(),
      isolated: await env.OBJECTS_OTHER.getByName("primary").snapshot(),
    },
  };
}

async function seed(env) {
  await env.CACHE.put("text", "seed-kv", { metadata: { stage: "seed", order: 1 } });
  await env.CACHE.put("json", JSON.stringify({ ok: true, product: "kv" }), { expirationTtl: 600 });
  await env.CACHE.put("binary", new Uint8Array([9, 1, 2, 9]).subarray(1, 3));
  await env.CACHE.put("stream", new ReadableStream({
    start(controller) {
      controller.enqueue(new TextEncoder().encode("stream-"));
      controller.enqueue(new TextEncoder().encode("value"));
      controller.close();
    },
  }));
  await env.CACHE_OTHER.put("text", "isolated-kv");

  await env.BUCKET.put("doc.txt", "hello-r2", {
    httpMetadata: { contentType: "text/plain", cacheControl: "max-age=60" },
    customMetadata: { stage: "seed" },
  });
  await env.BUCKET.put("binary.bin", new Uint8Array([3, 4, 5]));
  await env.BUCKET_OTHER.put("doc.txt", "isolated-r2");

  await env.DB.prepare("INSERT INTO notes(id, body) VALUES(?1, ?2)").bind(1, "seed-d1").run();
  await env.DB.batch([
    env.DB.prepare("INSERT INTO notes(id, body) VALUES(?1, ?2)").bind(2, "batch-a"),
    env.DB.prepare("INSERT INTO notes(id, body) VALUES(?1, ?2)").bind(3, "batch-b"),
  ]);
  await env.DB_OTHER.exec("CREATE TABLE notes(id INTEGER PRIMARY KEY, body TEXT NOT NULL)");
  await env.DB_OTHER.prepare("INSERT INTO notes(id, body) VALUES(1, 'isolated-d1')").run();

  await primaryStub(env).increment();
  await env.OBJECTS_OTHER.getByName("primary").increment();
  return snapshot(env);
}

async function websocketMatrix(env) {
  const response = await primaryStub(env).fetch(new Request("https://object.invalid/websocket", {
    headers: { Connection: "Upgrade", Upgrade: "websocket" },
  }));
  const socket = response.webSocket;
  if (!socket) throw new Error("missing websocket");
  socket.accept();
  const next = () => Promise.race([
    new Promise((resolve) => socket.addEventListener("message", (event) => resolve(event.data), { once: true })),
    scheduler.wait(2000).then(() => { throw new Error("websocket timeout"); }),
  ]);
  socket.send("ping");
  const text = await next();
  socket.send(new Uint8Array([1, 2, 3]));
  const binary = await next();
  socket.close(1000, "done");
  let bytes = binary instanceof ArrayBuffer
    ? new Uint8Array(binary)
    : ArrayBuffer.isView(binary)
      ? new Uint8Array(binary.buffer, binary.byteOffset, binary.byteLength)
      : null;
  if (!bytes && binary instanceof Blob) bytes = new Uint8Array(await binary.arrayBuffer());
  return { text: text === "ping", binary: Boolean(bytes) && Array.from(bytes).join(",") === "1,2,3" };
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    try {
      if (url.pathname === "/seed") return Response.json(await seed(env));
      if (url.pathname === "/snapshot") return Response.json(await snapshot(env));
      if (url.pathname === "/websocket") return Response.json(await websocketMatrix(env));
      if (url.pathname === "/set-alarm") {
        return new Response(String(await primaryStub(env).setAlarmAt(Number(url.searchParams.get("time")))));
      }
      if (url.pathname === "/alarm-status") return Response.json(await primaryStub(env).snapshot());
      if (url.pathname === "/mutate") {
        await env.CACHE.put("text", "mutated-kv", { metadata: { stage: "mutated" } });
        await env.DB.prepare("UPDATE notes SET body = 'mutated-d1' WHERE id = 1").run();
        return new Response("mutated");
      }
      if (url.pathname === "/s3-fault") {
        await scheduler.wait(Number(url.searchParams.get("delay") || 0));
        let r2Error = null;
        try { await env.BUCKET.head("doc.txt"); } catch (error) { r2Error = errorCode(error); }
        return Response.json({
          r2Error,
          kv: await env.CACHE.get("text", "text"),
          d1: await env.DB.prepare("SELECT body FROM notes WHERE id = 1").first("body"),
        });
      }
      if (url.pathname === "/corruption") {
        const primary = await env.DB.prepare("SELECT body FROM notes WHERE id = 1").first("body");
        let corruptError = null;
        try { await env.DB_CORRUPT.prepare("SELECT 1").first(); } catch (error) { corruptError = errorCode(error); }
        return Response.json({ primary, corruptError, kv: await env.CACHE.get("text", "text") });
      }
      if (url.pathname === "/delete-r2") {
        await env.BUCKET.delete(["doc.txt", "binary.bin"]);
        return new Response(String(await env.BUCKET.head("doc.txt")));
      }
      return new Response("missing", { status: 404 });
    } catch (error) {
      return new Response(error && error.stack ? error.stack : String(error), { status: 598 });
    }
  },
};
