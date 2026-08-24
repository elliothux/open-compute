import { logEvent, requestIdFrom } from "./log.js";

export default {
  async fetch(request) {
    const requestId = requestIdFrom(request);
    const url = new URL(request.url);
    if (url.pathname === "/throw" || url.searchParams.get("throw") === "1") {
      logEvent({
        requestId,
        dispatchKind: "fetch",
        entrypoint: "default",
        outcome: "error",
        errorCode: "ECHO_THROW",
      });
      throw new Error("g0-echo-throw");
    }
    logEvent({
      requestId,
      dispatchKind: "fetch",
      entrypoint: "default",
      outcome: "ok",
    });
    return Response.json({
      ok: true,
      service: "echo",
      entrypoint: "default",
      path: url.pathname,
    });
  },
};

export const named = {
  async fetch(request) {
    const requestId = requestIdFrom(request);
    logEvent({
      requestId,
      dispatchKind: "fetch",
      entrypoint: "named",
      outcome: "ok",
    });
    return Response.json({
      ok: true,
      service: "echo",
      entrypoint: "named",
    });
  },
};
