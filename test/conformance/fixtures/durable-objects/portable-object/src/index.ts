import { DurableObject } from "cloudflare:workers";

interface Env {
  OBJECTS: DurableObjectNamespace<PortableObject>;
}

interface FacetProps {
  marker: string;
}

interface SurfaceResult {
  storage: Record<string, boolean>;
  context: Record<string, boolean>;
  facets: {
    first: number;
    second: number;
    props: string;
    nestedSecond: number;
    clone: number;
    nestedClone: number;
    fresh: number;
    nestedFresh: number;
  };
}

function property(value: object, key: PropertyKey): unknown {
  return Reflect.get(value, key);
}

export class PortableFacet extends DurableObject<Record<string, never>, FacetProps> {
  async increment(): Promise<number> {
    const value = (await this.ctx.storage.get<number>("value")) ?? 0;
    await this.ctx.storage.put("value", value + 1);
    return value + 1;
  }

  props(): string {
    return this.ctx.props.marker;
  }

  childIncrement(): Promise<number> {
    const exports = this.ctx.exports as typeof this.ctx.exports & {
      PortableLeaf: LoopbackDurableObjectClass<PortableLeaf>;
    };
    const child = this.ctx.facets.get("leaf", () => ({
      class: exports.PortableLeaf({ props: { marker: "leaf" } }),
      id: "portable-leaf",
    }));
    return child.increment();
  }
}

export class PortableLeaf extends DurableObject<Record<string, never>, FacetProps> {
  async increment(): Promise<number> {
    const value = (await this.ctx.storage.get<number>("value")) ?? 0;
    await this.ctx.storage.put("value", value + 1);
    return value + 1;
  }
}

export class PortableObject extends DurableObject<Env> {
  async alarm(): Promise<void> {}

  async increment(): Promise<number> {
    const value = (await this.ctx.storage.get<number>("count")) ?? 0;
    await this.ctx.storage.put("count", value + 1);
    return value + 1;
  }

  echo(value: unknown): unknown {
    return value;
  }

  async surface(): Promise<SurfaceResult> {
    const storage = this.ctx.storage;
    await storage.put("async", { value: 1 }, { allowConcurrency: true, allowUnconfirmed: false, noCache: true });
    const asyncValue = await storage.get<{ value: number }>("async", { allowConcurrency: true, noCache: true });
    storage.kv.put("sync", { value: 2 });
    const syncValue = storage.kv.get<{ value: number }>("sync");
    await storage.transaction(async transaction => {
      await transaction.put("rollback", true);
      transaction.rollback();
    });
    const transactionRollback = await storage.get("rollback") === undefined;
    storage.sql.exec("CREATE TABLE IF NOT EXISTS portable(value INTEGER NOT NULL)");
    storage.sql.exec("DELETE FROM portable");
    storage.sql.exec("INSERT INTO portable(value) VALUES (?)", 7);
    const sql = storage.sql.exec<{ value: number }>("SELECT value FROM portable").one().value === 7;
    await storage.setAlarm(Date.now() + 86_400_000, { allowConcurrency: true });
    const alarm = typeof await storage.getAlarm({ allowConcurrency: true }) === "number";
    await storage.deleteAlarm({ allowConcurrency: true });
    const blockConcurrency = await this.ctx.blockConcurrencyWhile(async () => true);
    const waited = Promise.resolve(true);
    this.ctx.waitUntil(waited);
    const waitUntil = await waited;
    const exports = this.ctx.exports as typeof this.ctx.exports & {
      PortableFacet: LoopbackDurableObjectClass<PortableFacet>;
    };
    const facetClass = exports.PortableFacet({ props: { marker: "facet" } });
    const facet = this.ctx.facets.get("portable", () => ({ class: facetClass, id: "portable-facet" }));
    const first = await facet.increment();
    const second = await facet.increment();
    const props = await facet.props();
    await facet.childIncrement();
    const nestedSecond = await facet.childIncrement();
    this.ctx.facets.clone("portable", "portable-copy");
    const copy = this.ctx.facets.get("portable-copy", () => ({ class: facetClass, id: "unexpected" }));
    const clone = await copy.increment();
    const nestedClone = await copy.childIncrement();
    this.ctx.facets.delete("portable-copy");
    const freshCopy = this.ctx.facets.get(
      "portable-copy",
      () => ({ class: facetClass, id: "portable-fresh" }),
    );
    const fresh = await freshCopy.increment();
    const nestedFresh = await freshCopy.childIncrement();
    await storage.deleteAll({ allowUnconfirmed: false });
    const deleteAll = await storage.get("async") === undefined && storage.kv.get("sync") === undefined;
    storage.sql.exec("CREATE TABLE IF NOT EXISTS portable(value INTEGER NOT NULL)");
    return {
      storage: {
        asyncKv: asyncValue?.value === 1,
        syncKv: syncValue?.value === 2,
        transactionRollback,
        sql,
        alarm,
        deleteAll,
      },
      context: {
        id: typeof this.ctx.id.toString() === "string",
        props: this.ctx.props !== null && typeof this.ctx.props === "object",
        exports: this.ctx.exports !== undefined,
        blockConcurrency,
        waitUntil,
      },
      facets: { first, second, props, nestedSecond, clone, nestedClone, fresh, nestedFresh },
    };
  }

  async cleanup(): Promise<void> {
    await this.ctx.storage.deleteAll();
  }
}

function stub(env: Env): DurableObjectStub<PortableObject> {
  return env.OBJECTS.getByName("portable", { locationHint: "enam" });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/ids") {
      const scoped = env.OBJECTS.jurisdiction("eu");
      const named = scoped.idFromName("portable");
      const repeated = scoped.idFromName("portable");
      const parsed = scoped.idFromString(named.toString());
      const unique = scoped.newUniqueId({ jurisdiction: "eu" });
      const object = scoped.get(named, { locationHint: "enam" });
      return Response.json({
        namedStable: named.equals(repeated),
        parsedEqual: named.equals(parsed),
        namedName: named.name,
        uniqueNameAbsent: unique.name === undefined,
        stubIdentity: object.id.equals(named) && object.name === "portable",
        jurisdiction: named.jurisdiction,
      });
    }
    if (path === "/surface" && request.method === "POST") {
      const object = stub(env);
      const first = await object.increment();
      const count = first + await object.increment() - 1;
      const structured = await object.echo({ when: new Date(0), map: new Map([["x", new Set([1, 2])]]) });
      const result = await object.surface();
      return Response.json({
        rpc: {
          count,
          structured: structured !== null && typeof structured === "object"
            && property(structured, "when") instanceof Date
            && property(structured, "map") instanceof Map,
        },
        ...result,
      });
    }
    if (path === "/cleanup" && request.method === "DELETE") {
      await stub(env).cleanup();
      return Response.json({ cleaned: true });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
