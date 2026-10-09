/** Keep native HTTP/WebSocket delivery while acknowledging handler admission. */
export function createFetchAdmission(ctx: DurableObjectState) {
  const pending = new Map<
    string,
    { started: Rpc.Stub<() => Promise<void>>; abort: AbortController }
  >();
  const cancel = (token: string) => {
    const item = pending.get(token);
    pending.delete(token);
    item?.abort.abort();
    item?.started[Symbol.dispose]();
  };
  return {
    prepare(
      token: string,
      started: Rpc.Stub<() => Promise<void>>,
      timeoutMs: number,
    ): void {
      if (
        !/^[0-9a-f-]{36}$/.test(token) ||
        typeof started !== "function" ||
        typeof started.dup !== "function" ||
        !Number.isSafeInteger(timeoutMs) ||
        timeoutMs < 1 ||
        pending.has(token)
      )
        throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
      if (pending.size >= 128) throw new Error("DO_STORAGE_LIMIT");
      const item = { started: started.dup(), abort: new AbortController() };
      pending.set(token, item);
      ctx.waitUntil(
        scheduler
          .wait(timeoutMs, { signal: item.abort.signal })
          .catch(() => undefined)
          .then(() => {
            if (pending.get(token) === item) cancel(token);
          }),
      );
    },
    cancel,
    start(args: unknown[]): (() => Promise<void>) | undefined {
      const request = args[0];
      if (!(request instanceof Request)) return undefined;
      const token = request.headers.get("x-open-compute-fetch-admission");
      if (token === null) return undefined;
      const item = pending.get(token);
      if (!item) throw new Error("DO_RUNTIME_EXCEPTION");
      pending.delete(token);
      item.abort.abort();
      const headers = new Headers(request.headers);
      headers.delete("x-open-compute-fetch-admission");
      args[0] = new Request(request, { headers });
      return async () => {
        try {
          await item.started();
        } finally {
          item.started[Symbol.dispose]();
        }
      };
    },
  };
}
