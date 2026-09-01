interface Env {
  KV: KVNamespace;
}

interface ErrorObservation {
  synchronous: boolean;
  name: string;
  message: string;
}

function invoke(method: Function, owner: object, args: unknown[]): Promise<unknown> {
  return Reflect.apply(method, owner, args) as Promise<unknown>;
}

async function capture(call: () => Promise<unknown>): Promise<ErrorObservation | null> {
  let synchronous = true;
  try {
    const pending = call();
    synchronous = false;
    await pending;
    return null;
  } catch (error) {
    return {
      synchronous,
      name: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

async function reset(kv: KVNamespace): Promise<Response> {
  let cursor: string | undefined;
  do {
    const page = await kv.list({ prefix: "portable:", ...(cursor === undefined ? {} : { cursor }) });
    for (const item of page.keys) await kv.delete(item.name);
    cursor = page.list_complete ? undefined : page.cursor;
  } while (cursor !== undefined);
  return Response.json({ reset: true });
}

async function seed(kv: KVNamespace): Promise<Response> {
  await kv.put("portable:text", "hello", { metadata: { tag: "text" } });
  await kv.put("portable:json", JSON.stringify({ ok: true }), { metadata: { tag: "json" } });
  return Response.json({ seeded: true });
}

async function read(kv: KVNamespace): Promise<Response> {
  const bulkText = await kv.get(["portable:text", "portable:missing"]);
  const bulkJson = await kv.get<{ ok: boolean }>(["portable:json"], "json");
  const bulkMetadata = await kv.getWithMetadata<{ tag: string }>(
    ["portable:text", "portable:missing"],
  );
  const singleMetadata = await kv.getWithMetadata<{ tag: string }>("portable:text");
  const list = await kv.list<{ tag: string }>({ prefix: "portable:", limit: 10, cursor: null });
  return Response.json({
    bulk: [
      ["portable:text", bulkText.get("portable:text")],
      ["portable:json", bulkJson.get("portable:json")],
      ["portable:missing", bulkText.get("portable:missing")],
    ],
    bulkMetadata: [...bulkMetadata].map(([name, result]) => [name, result === null ? null : {
      value: result.value,
      metadata: result.metadata,
      hasCacheStatus: Object.hasOwn(result, "cacheStatus"),
    }]),
    singleMetadata: {
      value: singleMetadata.value,
      metadata: singleMetadata.metadata,
      hasCacheStatus: Object.hasOwn(singleMetadata, "cacheStatus"),
      cacheStatusValid: singleMetadata.cacheStatus === null || typeof singleMetadata.cacheStatus === "string",
    },
    list: {
      keys: list.keys.map(item => ({
        name: item.name,
        metadata: item.metadata,
        hasExpiration: Object.hasOwn(item, "expiration"),
      })),
      list_complete: list.list_complete,
      hasCursor: Object.hasOwn(list, "cursor"),
      cacheStatusValid: list.cacheStatus === null || typeof list.cacheStatus === "string",
    },
  });
}

async function errors(kv: KVNamespace): Promise<Response> {
  const cases: Record<string, () => Promise<unknown>> = {
    getEmpty: () => invoke(kv.get, kv, [""]),
    deleteDotDot: () => invoke(kv.delete, kv, [".."]),
    getLong: () => invoke(kv.get, kv, ["x".repeat(513)]),
    getNumber: () => invoke(kv.get, kv, [1]),
    getUnpairedSurrogate: () => invoke(kv.get, kv, ["\ud800"]),
    getInvalidType: () => invoke(kv.get, kv, ["portable:text", "banana"]),
    cacheTtlLow: () => invoke(kv.get, kv, ["portable:text", { cacheTtl: 29 }]),
    getUnknownOption: () => invoke(kv.get, kv, ["portable:text", { unknown: true }]),
    bulkEmpty: () => invoke(kv.get, kv, [[]]),
    bulkEmptyKey: () => invoke(kv.get, kv, [[""]]),
    bulkDotKey: () => invoke(kv.get, kv, [["."]]),
    bulkLongKey: () => invoke(kv.get, kv, [["x".repeat(513)]]),
    bulkUnpairedSurrogate: () => invoke(kv.get, kv, [["\ud800"]]),
    bulkMetadataDotDotKey: () => invoke(kv.getWithMetadata, kv, [[".."]]),
    bulkTooMany: () => invoke(kv.get, kv, [Array.from({ length: 101 }, (_, index) => `k${index}`)]),
    bulkStream: () => invoke(kv.get, kv, [["portable:text"], "stream"]),
    putInvalidValue: () => invoke(kv.put, kv, ["portable:invalid", { value: true }]),
    putBothExpiration: () => invoke(kv.put, kv, [
      "portable:both", "value", { expiration: 1, expirationTtl: 60 },
    ]),
    putTtlLow: () => invoke(kv.put, kv, ["portable:ttl", "value", { expirationTtl: 59 }]),
    putMetadataTooLarge: () => invoke(kv.put, kv, [
      "portable:metadata", "value", { metadata: { value: "x".repeat(1024) } },
    ]),
    listZero: () => invoke(kv.list, kv, [{ limit: 0 }]),
    listHigh: () => invoke(kv.list, kv, [{ limit: 1001 }]),
    listNumberPrefix: () => invoke(kv.list, kv, [{ prefix: 1 }]),
    listUnpairedSurrogate: () => invoke(kv.list, kv, [{ prefix: "\ud800" }]),
    listUnknownOption: () => invoke(kv.list, kv, [{ unknown: true }]),
  };
  const observed: Record<string, ErrorObservation | null> = {};
  for (const [name, call] of Object.entries(cases)) observed[name] = await capture(call);
  return Response.json(observed);
}

async function stream(kv: KVNamespace): Promise<Response> {
  const value = new ReadableStream({
    start(controller) {
      controller.enqueue("stream-");
      controller.enqueue(new TextEncoder().encode("value"));
      controller.close();
    },
  });
  await invoke(kv.put, kv, ["portable:stream", value]);
  return Response.json({ value: await kv.get("portable:stream") });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/reset") return reset(env.KV);
    if (path === "/seed") return seed(env.KV);
    if (path === "/read") return read(env.KV);
    if (path === "/errors") return errors(env.KV);
    if (path === "/stream") return stream(env.KV);
    if (path === "/delete") {
      await env.KV.delete("portable:absent");
      return Response.json({ deleted: true, value: await env.KV.get("portable:absent") });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
