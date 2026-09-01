async function streamSurface(): Promise<Record<string, string>> {
  const transformed = new Response(new Blob(["portable"]).stream()
    .pipeThrough(new TextDecoderStream())
    .pipeThrough(new TransformStream<string, string>({ transform(chunk, controller) {
      controller.enqueue(chunk.toUpperCase());
    } }))
    .pipeThrough(new TextEncoderStream()));
  const compressed = new Response("portable").body!
    .pipeThrough(new CompressionStream("gzip"))
    .pipeThrough(new DecompressionStream("gzip"));
  const fixed = new FixedLengthStream(5);
  const writing = (async () => {
    const writer = fixed.writable.getWriter();
    await writer.write(new TextEncoder().encode("hello"));
    await writer.close();
  })();
  const fixedLength = new Response(fixed.readable).text();
  await writing;
  return {
    transform: await transformed.text(),
    compression: await new Response(compressed).text(),
    fixedLength: await fixedLength,
  };
}

async function eventSurface(): Promise<Record<string, unknown>> {
  const target = new EventTarget();
  let custom = "";
  target.addEventListener("portable", event => {
    if (event instanceof CustomEvent) custom = String(event.detail);
  }, { once: true });
  target.dispatchEvent(new CustomEvent("portable", { detail: "portable" }));
  const channel = new MessageChannel();
  const message = new Promise<unknown>(resolve => channel.port1.addEventListener("message", event => {
    resolve(event instanceof MessageEvent ? event.data : undefined);
  }, { once: true }));
  channel.port1.start();
  channel.port2.postMessage("portable");
  const controller = new AbortController();
  controller.abort(new DOMException("portable", "AbortError"));
  return { custom, message: await message, aborted: controller.signal.aborted };
}

async function runtime(request: Request, ctx: ExecutionContext): Promise<Response> {
  const url = new URL(request.url);
  const headers = new Headers(request.headers);
  headers.append("x-roundtrip", "one");
  headers.set("x-roundtrip", "two");
  const pattern = new URLPattern({ pathname: "/runtime" });
  const cloned = request.clone();
  const body = await cloned.text();
  const encoded = new TextEncoder().encode("portable");
  const decoded = new TextDecoder().decode(encoded);
  const digest = Buffer.from(await crypto.subtle.digest("SHA-256", encoded)).toString("hex");
  const moduleBytes = Uint8Array.from([0, 97, 115, 109, 1, 0, 0, 0]);
  const rewritten = await new HTMLRewriter().on("p", {
    element(element) { element.setInnerContent("portable"); },
  }).transform(new Response("<p>replace</p>")).text();
  const pair = new WebSocketPair();
  const response = Response.json({ ok: true }, { headers: { "x-response": "portable" } });
  const responseClone = response.clone();
  const responseBody: unknown = await response.json();
  const before = performance.now();
  await scheduler.wait(1);
  const after = performance.now();
  return Response.json({
    fetch: {
      request: request.method === "POST" && request instanceof Request,
      headers: headers.get("x-portable") === "yes" && headers.get("x-roundtrip") === "two",
      body,
      url: url.searchParams.get("a") === "1" && new URLSearchParams(url.search).size === 2,
      pattern: pattern.test(url) && pattern.exec(url)?.pathname.input === "/runtime",
      response: responseClone instanceof Response && responseClone.headers.get("x-response") === "portable"
        && responseBody !== null && typeof responseBody === "object" && Reflect.get(responseBody, "ok") === true,
    },
    binary: {
      text: decoded,
      digest,
      base64: Buffer.from(encoded).toString("base64"),
      webAssembly: WebAssembly.validate(moduleBytes),
    },
    streams: await streamSurface(),
    events: await eventSurface(),
    html: rewritten,
    runtime: {
      uuid: /^[0-9a-f-]{36}$/.test(crypto.randomUUID()),
      performance: Number.isFinite(before) && after >= before,
      navigator: typeof navigator.userAgent === "string" && navigator.userAgent.length > 0,
      exports: ctx.exports !== undefined,
      props: ctx.props !== null && typeof ctx.props === "object",
      webSocketPair: pair[0] instanceof WebSocket && pair[1] instanceof WebSocket,
    },
  });
}

export default {
  async fetch(request: Request, _env: Record<string, never>, ctx: ExecutionContext): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/runtime" && request.method === "POST") return runtime(request, ctx);
    if (path === "/cleanup" && request.method === "DELETE") return Response.json({ cleaned: true });
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Record<string, never>>;
