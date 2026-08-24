import { value } from "./dep.js";

let abortEvents = 0;

function json(data, status = 200) {
  return Response.json(data, { status });
}

function observeAbort(signal) {
  if (!signal) return;
  const mark = () => {
    abortEvents += 1;
  };
  if (signal.aborted) mark();
  else signal.addEventListener("abort", mark, { once: true });
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (url.pathname === "/abort-status") {
      return json({ deployment: "A", abortEvents });
    }
    if (url.pathname === "/stream") {
      const encoder = new TextEncoder();
      const stream = new ReadableStream({
        start(controller) {
          controller.enqueue(encoder.encode("chunk-a-1"));
          controller.enqueue(encoder.encode("chunk-a-2"));
          controller.close();
        },
      });
      return new Response(stream, { headers: { "content-type": "text/plain" } });
    }
    if (url.pathname === "/body") {
      const text = await request.text();
      return json({ deployment: "A", body: text, module: value });
    }
    if (url.pathname === "/hang") {
      observeAbort(request.signal);
      const delay = (ms) =>
        typeof scheduler !== "undefined" && scheduler.wait
          ? scheduler.wait(ms)
          : new Promise((resolve) => setTimeout(resolve, ms));
      const aborted = new Promise((_, reject) => {
        const fail = () => {
          const err = new Error("aborted");
          err.name = "AbortError";
          reject(err);
        };
        if (request.signal?.aborted) fail();
        else request.signal?.addEventListener("abort", fail, { once: true });
      });
      try {
        await Promise.race([delay(10000), aborted]);
        return json({ hang: "timeout" });
      } catch (err) {
        if (err && (err.name === "AbortError" || request.signal?.aborted)) {
          return json({ hang: "aborted" }, 499);
        }
        throw err;
      }
    }
    if (url.pathname === "/headers") {
      return json({
        deployment: "A",
        module: value,
        accountHeader: request.headers.get("x-account-id"),
        deploymentHeader: request.headers.get("x-deployment-id"),
        envDeployment: env.G0_IDENTITY?.deploymentId ?? null,
      });
    }
    return json({
      deployment: "A",
      module: value,
      entrypoint: "default",
      identity: env.G0_IDENTITY ?? null,
    });
  },
};

export const extra = {
  async fetch(request, env) {
    return Response.json({
      deployment: "A",
      module: value,
      entrypoint: "extra",
      identity: env.G0_IDENTITY ?? null,
    });
  },
};
