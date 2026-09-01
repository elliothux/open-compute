interface Env {
  KV: KVNamespace;
  TYPED: KVNamespace<"alpha" | "beta">;
}

async function exerciseOverloads(kv: KVNamespace, typed: KVNamespace<"alpha" | "beta">): Promise<unknown[]> {
  const observed: unknown[] = [];
  observed.push(await kv.get("k"));
  observed.push(await kv.get("k", "text"));
  observed.push(await kv.get<{ ok: boolean }>("k", "json"));
  observed.push(await kv.get("k", "arrayBuffer"));
  observed.push(await kv.get("k", "stream"));
  observed.push(await kv.get("k", { type: "text" }));
  observed.push(await kv.get<{ ok: boolean }>("k", { type: "json", cacheTtl: 60 }));
  observed.push(await kv.get("k", { type: "arrayBuffer" }));
  observed.push(await kv.get("k", { type: "stream" }));
  observed.push(await kv.get("k", { cacheTtl: 60 }));
  observed.push(await kv.get(["a", "b"]));
  observed.push(await kv.get(["a", "b"], "text"));
  observed.push(await kv.get<{ ok: boolean }>(["a", "b"], "json"));
  observed.push(await kv.get(["a", "b"], { type: "text" }));
  observed.push(await kv.get<{ ok: boolean }>(["a", "b"], { type: "json" }));
  observed.push(await kv.get(["a", "b"], { cacheTtl: 60 }));

  const meta = await kv.getWithMetadata<{ owner: string }>("k");
  const metaText: string | null = meta.value;
  const metaData: { owner: string } | null = meta.metadata;
  const metaCache: string | null = meta.cacheStatus;
  observed.push(metaText, metaData, metaCache);
  observed.push(await kv.getWithMetadata("k", "text"));
  observed.push(await kv.getWithMetadata<{ ok: boolean }, { owner: string }>("k", "json"));
  observed.push(await kv.getWithMetadata("k", "arrayBuffer"));
  observed.push(await kv.getWithMetadata("k", "stream"));
  observed.push(await kv.getWithMetadata("k", { type: "text" }));
  observed.push(await kv.getWithMetadata<{ ok: boolean }>("k", { type: "json" }));
  observed.push(await kv.getWithMetadata("k", { type: "arrayBuffer" }));
  observed.push(await kv.getWithMetadata("k", { type: "stream" }));
  observed.push(await kv.getWithMetadata(["a", "b"], "text"));
  observed.push(await kv.getWithMetadata<{ ok: boolean }>(["a", "b"], "json"));
  observed.push(await kv.getWithMetadata(["a", "b"]));
  observed.push(await kv.getWithMetadata(["a", "b"], { type: "text" }));
  observed.push(await kv.getWithMetadata<{ ok: boolean }>(["a", "b"], { type: "json" }));

  const bulk = await kv.get(["a", "b"], "text");
  const bulkValue: string | null | undefined = bulk.get("a");
  observed.push(bulk.size, bulkValue);

  await kv.put("k", "value");
  await kv.put("k", new ArrayBuffer(1));
  await kv.put("k", new Uint8Array(1));
  await kv.put("k", new ReadableStream());
  await kv.put("k", "value", { expiration: 1_700_000_000, metadata: null });
  await kv.put("k", "value", { expirationTtl: 60, metadata: { a: 1 } });
  await kv.delete("k");

  const listed = await kv.list<{ tag: string }>({ prefix: null, cursor: null, limit: 10 });
  const keys: KVNamespaceListKey<{ tag: string }>[] = listed.keys;
  const complete: boolean = listed.list_complete;
  const listCache: string | null = listed.cacheStatus;
  observed.push(keys, complete, listCache);
  if (listed.list_complete) {
    // @ts-expect-error complete pages have no cursor
    const _missing: string = listed.cursor;
    observed.push(_missing);
  } else {
    const cursor: string = listed.cursor;
    observed.push(cursor);
  }

  const typedGet: string | null = await typed.get("alpha");
  await typed.put("beta", "value");
  await typed.delete("alpha");
  const typedList = await typed.list();
  for (const key of typedList.keys) {
    const name: "alpha" | "beta" = key.name;
    observed.push(name);
  }
  observed.push(typedGet);

  // @ts-expect-error
  await typed.get("gamma");
  // @ts-expect-error
  await typed.put("other", "value");
  // @ts-expect-error
  await kv.get("k", "banana");
  // @ts-expect-error
  await kv.get(["a"], "arrayBuffer");
  // @ts-expect-error
  await kv.get(["a"], "stream");
  // @ts-expect-error
  await kv.put("k", { obj: true });
  // @ts-expect-error
  await kv.list({ prefix: 1 });
  // @ts-expect-error
  await kv.list({ cursor: 1 });
  return observed;
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const observed = await exerciseOverloads(env.KV, env.TYPED);
    return Response.json({ count: observed.length });
  },
} satisfies ExportedHandler<Env>;
