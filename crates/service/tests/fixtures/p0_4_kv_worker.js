export default {
  async fetch(request, env) {
    const path = new URL(request.url).pathname;
    const code = async (run) => {
      let synchronous = true;
      try {
        const pending = run();
        synchronous = false;
        await pending;
        return null;
      } catch (error) {
        return {
          synchronous,
          name: error && error.name ? error.name : typeof error,
          message: String(error && error.message ? error.message : error),
        };
      }
    };
    if (path === "/seed") {
      await env.CACHE.put("text", "hello", { metadata: { z: 2, a: 1 } });
      await env.CACHE.put("json", JSON.stringify({ ok: true }), { expirationTtl: 60 });
      const view = new Uint8Array([9, 255, 1, 8]).subarray(1, 3);
      await env.CACHE.put("binary", view);
      await env.CACHE.put("stream", new ReadableStream({
        start(controller) {
          controller.enqueue(new TextEncoder().encode("stream-"));
          controller.enqueue(new TextEncoder().encode("value"));
          controller.close();
        }
      }));
      await env.CACHE.put("expiring", "ttl", { expiration: Math.floor(Date.now() / 1000) + 120 });
      await env.CACHE.put("bad-json", "{");
      await env.OTHER.put("text", "isolated");
      return new Response("seeded");
    }
    if (path === "/snapshot") {
      try {
        const withMetadata = await env.CACHE.getWithMetadata("text");
        const typedText = await env.CACHE.getWithMetadata("text", "text");
        const typedJson = await env.CACHE.getWithMetadata("json", { type: "json" });
        const binary = Array.from(new Uint8Array(await env.CACHE.get("binary", "arrayBuffer")));
        const stream = await new Response(await env.CACHE.get("stream", "stream")).text();
        const many = Array.from((await env.CACHE.get(["text", "missing", "text"])).entries());
        const manyMeta = Array.from((await env.CACHE.getWithMetadata(["text", "missing"], { type: "text" })).entries());
        return Response.json({
          text: withMetadata.value,
          metadata: withMetadata.metadata,
          cacheStatus: withMetadata.cacheStatus,
          typedText: typedText.value,
          typedJson: typedJson.value,
          json: await env.CACHE.get("json", "json"),
          optionText: await env.CACHE.get("text", { type: "text", cacheTtl: 30 }),
          binary,
          stream,
          other: await env.OTHER.get("text"),
          many,
          manyMeta,
        });
      } catch (error) {
        return new Response(String(error && error.stack ? error.stack : error), { status: 599 });
      }
    }
    if (path === "/large") {
      const streamOf = (bytes) => {
        let emitted = 0;
        return new ReadableStream({
          pull(controller) {
            if (emitted >= bytes) { controller.close(); return; }
            const size = Math.min(1024 * 1024, bytes - emitted);
            const chunk = new Uint8Array(size);
            chunk.fill(7);
            emitted += size;
            controller.enqueue(chunk);
          }
        });
      };
      const limit = 25 * 1024 * 1024;
      await env.CACHE.put("large", streamOf(limit));
      let rejected = false;
      try { await env.CACHE.put("large", streamOf(limit + 1)); } catch { rejected = true; }
      const reader = (await env.CACHE.get("large", "stream")).getReader();
      let total = 0;
      let first = null;
      let last = null;
      for (;;) {
        const next = await reader.read();
        if (next.done) break;
        if (first === null && next.value.byteLength) first = next.value[0];
        if (next.value.byteLength) last = next.value[next.value.byteLength - 1];
        total += next.value.byteLength;
      }
      return new Response(`${total}:${first}:${last}:${rejected}`);
    }
    if (path === "/cancel") {
      const reader = (await env.CACHE.get("large", "stream")).getReader();
      const first = await reader.read();
      if (first.done || first.value.byteLength === 0) throw new Error("empty stream");
      await reader.cancel("tenant cancelled");
      return new Response("cancelled");
    }
    if (path === "/page1") return Response.json(await env.CACHE.list({ limit: 1 }));
    if (path === "/page2") {
      try {
        return Response.json(await env.CACHE.list({ limit: 1, cursor: await request.text() }));
      } catch (error) {
        return new Response(String(error && error.message ? error.message : error), { status: 599 });
      }
    }
    if (path === "/list-complete") return Response.json(await env.CACHE.list({ prefix: null, cursor: null }));
    if (path === "/list-expiring") {
      const listed = await env.CACHE.list({ prefix: "expir" });
      return Response.json({
        name: listed.keys[0] && listed.keys[0].name,
        hasExpiration: typeof (listed.keys[0] && listed.keys[0].expiration) === "number",
        list_complete: listed.list_complete,
        cacheStatus: listed.cacheStatus,
        hasCursor: Object.prototype.hasOwnProperty.call(listed, "cursor"),
      });
    }
    if (path === "/failures") {
      const resizable = new ArrayBuffer(4, { maxByteLength: 16 });
      new Uint8Array(resizable).set([9, 8, 7, 6]);
      const pending = env.CACHE.put("rab", resizable);
      resizable.resize(0);
      await pending;
      let sab = null;
      if (typeof SharedArrayBuffer === "function") {
        const shared = new SharedArrayBuffer(3);
        const view = new Uint8Array(shared);
        view.set([1, 2, 3]);
        await env.CACHE.put("sab", view);
        view.set([9, 9, 9]);
        sab = Array.from(new Uint8Array(await env.CACHE.get("sab", "arrayBuffer")));
      }
      const rab = Array.from(new Uint8Array(await env.CACHE.get("rab", "arrayBuffer")));
      const detached = new ArrayBuffer(4);
      if (typeof detached.transfer === "function") detached.transfer();
      else structuredClone(detached, { transfer: [detached] });
      let jsonError = null;
      try { await env.CACHE.get("bad-json", "json"); } catch (error) {
        jsonError = error && error.name ? error.name : "Error";
      }
      return Response.json({
        emptyKey: await code(() => env.CACHE.get("")),
        dot: await code(() => env.CACHE.put(".", "x")),
        dotDot: await code(() => env.CACHE.delete("..")),
        longKey: await code(() => env.CACHE.get("x".repeat(513))),
        utf16: await code(() => env.CACHE.get("\uD800")),
        numberKey: await code(() => env.CACHE.get(1)),
        emptyBulk: await code(() => env.CACHE.get([])),
        emptyBulkKey: await code(() => env.CACHE.get([""])),
        dotBulkKey: await code(() => env.CACHE.get(["."])),
        longBulkKey: await code(() => env.CACHE.get(["x".repeat(513)])),
        invalidMetadataBulkKey: await code(() => env.CACHE.getWithMetadata([".."])),
        utf16Bulk: await code(() => env.CACHE.get(["\uD800"])),
        tooMany: await code(() => env.CACHE.get(Array.from({ length: 101 }, (_, i) => `k${i}`))),
        invalidType: await code(() => env.CACHE.get("text", "banana")),
        bulkStream: await code(() => env.CACHE.get(["text"], "stream")),
        cacheTtl: await code(() => env.CACHE.get("text", { cacheTtl: 29 })),
        bothExpiration: await code(() => env.CACHE.put("x", "y", { expiration: 10, expirationTtl: 60 })),
        ttlLow: await code(() => env.CACHE.put("x", "y", { expirationTtl: 59 })),
        objectValue: await code(() => env.CACHE.put("x", { obj: true })),
        detached: await code(() => env.CACHE.put("x", detached)),
        extraList: await code(() => env.CACHE.list({ extra: true })),
        zeroList: await code(() => env.CACHE.list({ limit: 0 })),
        highList: await code(() => env.CACHE.list({ limit: 1001 })),
        numberPrefix: await code(() => env.CACHE.list({ prefix: 1 })),
        utf16Prefix: await code(() => env.CACHE.list({ prefix: "\uD800" })),
        jsonError,
        rab,
        sab,
        readOnlyPut: await code(() => env.READONLY.put("denied", "no")),
        readOnlyGet: await env.READONLY.get("missing"),
      });
    }
    if (path === "/delete") { await env.CACHE.delete("text"); return new Response("deleted"); }
    if (path === "/missing") return new Response(String(await env.CACHE.get("text")));
    return new Response("missing", { status: 404 });
  }
};
