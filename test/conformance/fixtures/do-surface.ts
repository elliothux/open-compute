import { DurableObject } from "cloudflare:workers";

interface Env {
  OBJECTS: DurableObjectNamespace<Counter>;
}

export class Counter extends DurableObject {
  async echo(value: unknown): Promise<unknown> {
    return value;
  }

  async storageSurface(ws: WebSocket): Promise<void> {
    const storage = this.ctx.storage;
    await storage.put("one", { value: 1 }, { allowConcurrency: true, allowUnconfirmed: false, noCache: true });
    await storage.put({ two: 2, three: 3 });
    const one = await storage.get<{ value: number }>("one", { allowConcurrency: true, noCache: true });
    const many = await storage.get<number>(["two", "three"]);
    const listed = await storage.list<number>({
      start: "a", startAfter: "a", end: "z", prefix: "", reverse: true, limit: 10,
      allowConcurrency: true, noCache: true,
    });
    await storage.transaction(async transaction => {
      await transaction.put("four", 4, { allowUnconfirmed: true });
      await transaction.get("four");
      await transaction.list({ prefix: "f" });
      await transaction.setAlarm(new Date(), { allowConcurrency: true });
      await transaction.getAlarm();
      await transaction.deleteAlarm();
      transaction.rollback();
    });
    storage.transactionSync(() => {
      storage.kv.put("sync", { ok: true });
      const sync = storage.kv.get<{ ok: boolean }>("sync");
      const syncList = [...storage.kv.list({ prefix: "s", reverse: false, limit: 1 })];
      void sync;
      void syncList;
      storage.kv.delete("sync");
    });
    const cursor = storage.sql.exec<{ value: number }>("SELECT 1 AS value");
    const rows = cursor.toArray();
    const columns = cursor.columnNames;
    const size = storage.sql.databaseSize;
    await storage.setAlarm(Date.now());
    await storage.getAlarm();
    await storage.deleteAlarm();
    await storage.sync();
    const current = await storage.getCurrentBookmark();
    const historical = await storage.getBookmarkForTime(new Date());
    await storage.onNextSessionRestoreBookmark(current);
    await storage.delete(["two", "three"]);
    await storage.deleteAll();

    this.ctx.acceptWebSocket(ws, ["tag"]);
    const sockets = this.ctx.getWebSockets("tag");
    this.ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
    const auto = this.ctx.getWebSocketAutoResponse();
    const timestamp = this.ctx.getWebSocketAutoResponseTimestamp(ws);
    this.ctx.setHibernatableWebSocketEventTimeout(1000);
    const timeout = this.ctx.getHibernatableWebSocketEventTimeout();
    const tags = this.ctx.getTags(ws);
    ws.serializeAttachment({ current });
    const attachment = ws.deserializeAttachment();
    this.ctx.waitUntil(Promise.resolve());
    await this.ctx.blockConcurrencyWhile(async () => undefined);
    const exports = this.ctx.exports as typeof this.ctx.exports & {
      Facet: LoopbackDurableObjectClass<Facet>;
    };
    const facet = this.ctx.facets.get("facet", () => ({
      class: exports.Facet({ props: { marker: "facet" } }),
      id: "facet-id",
    }));
    await facet.echo("value");
    this.ctx.facets.clone("facet", "copy");
    this.ctx.facets.abort("facet", new Error("stop"));
    this.ctx.facets.delete("copy");
    void one;
    void many;
    void listed;
    void rows;
    void columns;
    void size;
    void historical;
    void sockets;
    void auto;
    void timestamp;
    void timeout;
    void tags;
    void attachment;
  }

  async alarm(_alarmInfo?: AlarmInvocationInfo): Promise<void> {}
  async webSocketMessage(_ws: WebSocket, _message: string | ArrayBuffer): Promise<void> {}
  async webSocketClose(_ws: WebSocket, _code: number, _reason: string, _wasClean: boolean): Promise<void> {}
  async webSocketError(_ws: WebSocket, _error: unknown): Promise<void> {}
}

export class Facet extends DurableObject<Record<string, never>, { marker: string }> {
  echo(value: string): string { return `${this.ctx.props.marker}:${value}`; }
}

export default {
  async fetch(_request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const eu = env.OBJECTS.jurisdiction("eu");
    const named = eu.idFromName("alpha");
    const unique = eu.newUniqueId({ jurisdiction: "eu" });
    const stub = eu.get(named, { locationHint: "enam", routingMode: "primary-only" });
    const byName = env.OBJECTS.getByName("alpha", { locationHint: "wnam" });
    ctx.waitUntil(Promise.resolve());
    return Response.json({
      jurisdiction: named.jurisdiction,
      unique: unique.toString(),
      equals: named.equals(eu.idFromString(named.toString())),
      id: stub.id.toString(),
      name: byName.name,
      exports: ctx.exports !== undefined,
    });
  },
} satisfies ExportedHandler<Env>;
