import {
  attachServiceWebSocketHandoffs,
  completeServiceScope,
} from "../../services/facade.js";
import type { Environment, TrackedContext } from "./types.js";

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve: () => void = () => {};
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function wrapRootStream(
  stream: ReadableStream<Uint8Array>,
  done: () => void,
): ReadableStream<Uint8Array> {
  const reader = stream.getReader();
  let finished = false;
  const finish = () => {
    if (!finished) {
      finished = true;
      done();
    }
  };
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const part = await reader.read();
        if (part.done) {
          finish();
          controller.close();
        } else controller.enqueue(part.value);
      } catch (error) {
        finish();
        controller.error(error);
      }
    },
    async cancel(reason) {
      try {
        await reader.cancel(reason);
      } finally {
        finish();
      }
    },
  });
}

function wrapRootWritable(
  stream: WritableStream<unknown>,
  done: () => void,
): WritableStream<unknown> {
  const writer = stream.getWriter();
  let finished = false;
  const finish = () => {
    if (!finished) {
      finished = true;
      done();
    }
  };
  writer.closed.then(finish, finish);
  return new WritableStream<unknown>({
    write(chunk) {
      return writer.write(chunk);
    },
    async close() {
      try {
        await writer.close();
      } finally {
        finish();
      }
    },
    async abort(reason) {
      try {
        await writer.abort(reason);
      } finally {
        finish();
      }
    },
  });
}

export function resultDrain(value: unknown): {
  value: unknown;
  drained: Promise<void>;
  handoffWebSocket: boolean;
} {
  if (value instanceof Response) {
    if (value.webSocket) {
      return {
        value: attachServiceWebSocketHandoffs(value),
        drained: Promise.resolve(),
        handoffWebSocket: true,
      };
    }
    if (!value.body)
      return {
        value: attachServiceWebSocketHandoffs(value),
        drained: Promise.resolve(),
        handoffWebSocket: false,
      };
    const end = deferred();
    return {
      value: attachServiceWebSocketHandoffs(
        new Response(wrapRootStream(value.body, end.resolve), {
          status: value.status,
          statusText: value.statusText,
          headers: value.headers,
        }),
      ),
      drained: end.promise,
      handoffWebSocket: false,
    };
  }
  if (value instanceof ReadableStream) {
    const end = deferred();
    return {
      value: wrapRootStream(value, end.resolve),
      drained: end.promise,
      handoffWebSocket: false,
    };
  }
  if (value instanceof WritableStream) {
    const end = deferred();
    return {
      value: wrapRootWritable(value, end.resolve),
      drained: end.promise,
      handoffWebSocket: false,
    };
  }
  if (value instanceof Request) {
    if (!value.body)
      return { value, drained: Promise.resolve(), handoffWebSocket: false };
    const end = deferred();
    return {
      value: new Request(value, {
        body: wrapRootStream(value.body, end.resolve),
      }),
      drained: end.promise,
      handoffWebSocket: false,
    };
  }
  return { value, drained: Promise.resolve(), handoffWebSocket: false };
}

export function scheduleRootCompletion(
  env: Environment,
  scopeId: string,
  tracked?: TrackedContext,
  drained: Promise<void> = Promise.resolve(),
): void {
  const background = tracked ? drainTrackedTasks(tracked) : Promise.resolve();
  const completion = Promise.all([background, drained])
    .then(() => completeServiceScope(env, scopeId))
    .catch(() => undefined);
  if (tracked) tracked.extendLifetime(completion);
}

export async function drainTrackedTasks(
  tracked: TrackedContext,
): Promise<void> {
  let consumed = 0;
  while (consumed < tracked.tasks.length) {
    const pending = tracked.tasks.slice(consumed);
    consumed = tracked.tasks.length;
    await Promise.allSettled(pending);
  }
}

export function rootResult(
  raw: unknown,
  env: Environment,
  scopeId: string,
  tracked?: TrackedContext,
): unknown {
  if (raw instanceof Promise) {
    return raw.then(
      (value) => {
        const result = resultDrain(value);
        scheduleRootCompletion(env, scopeId, tracked, result.drained);
        return result.value;
      },
      (error) => {
        scheduleRootCompletion(env, scopeId, tracked);
        throw error;
      },
    );
  }
  const result = resultDrain(raw);
  scheduleRootCompletion(env, scopeId, tracked, result.drained);
  return result.value;
}

function backgroundSignal(tracked: TrackedContext): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    async start(controller) {
      await drainTrackedTasks(tracked);
      controller.close();
    },
  });
}

export function serviceSuccess(
  value: unknown,
  tracked: TrackedContext,
): unknown {
  return Object.freeze({
    ok: true,
    value,
    background: backgroundSignal(tracked),
  });
}

export function serviceFailure(
  error: unknown,
  tracked: TrackedContext,
): unknown {
  return Object.freeze({
    ok: false,
    error,
    background: backgroundSignal(tracked),
  });
}
