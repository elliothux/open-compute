import { RpcTarget, waitUntil } from "cloudflare:workers";
import type { BindingEnv } from "../bindings/protocol.js";
import { bindingError } from "../loader/shared.js";
import { activateServiceCapabilities } from "./capability-transfer.js";
import type { ServiceFrame } from "./scope.js";

/** Private receipt for an admitted capability operation. */
export interface CapabilityAdmission {
  handle: string;
  frame: string;
  deadlineMs: number;
}
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
/** Validate a Service invocation identity at the private host boundary. */
export function serviceFrame(value: unknown): value is ServiceFrame {
  return (
    record(value) &&
    typeof value.scopeId === "string" &&
    (value.parentFrame === null || typeof value.parentFrame === "string") &&
    /^[0-9a-f-]{36}$/.test(value.scopeId) &&
    (value.parentFrame === null || /^[0-9a-f-]{36}$/.test(value.parentFrame))
  );
}

/** Send a private authority request with the current host generation. */
export async function serviceControl<T>(
  env: BindingEnv,
  path: string,
  body: unknown,
): Promise<T> {
  const response = await env.BINDING_BACKEND.fetch(
    `http://binding-backend${path}`,
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify(body),
    },
  );
  if (!response.ok) {
    throw bindingError(
      response.headers.get("x-open-compute-error-code") ||
        "SERVICE_UNAVAILABLE",
    );
  }
  const value: unknown = await response.json();
  return value as T;
}

/** Hold the canonical native WebSocket lease on one private streaming HTTP response. */
export async function holdServiceWebSocketHandoff(
  env: BindingEnv,
  handles: readonly string[],
): Promise<ReadableStream<Uint8Array>> {
  const response = await env.BINDING_BACKEND.fetch(
    "http://binding-backend/internal/services/v1/websocket/handoff",
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
      },
      body: JSON.stringify({ handles }),
    },
  );
  if (
    response.status !== 200 ||
    response.body === null ||
    response.headers.get("content-type") !== "application/octet-stream"
  ) {
    await response.body?.cancel();
    throw bindingError("SERVICE_UNAVAILABLE");
  }
  return response.body;
}

/** Retry one idempotent lifecycle mutation across a transient private-hop failure. */
export async function retryServiceControl<T>(
  env: BindingEnv,
  path: string,
  body: unknown,
): Promise<T> {
  let lastFailure: unknown;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      return await serviceControl(env, path, body);
    } catch (error) {
      lastFailure = error;
      if (attempt < 2) await scheduler.wait(10 * (attempt + 1));
    }
  }
  throw lastFailure;
}

/** Private completion and retention authority passed to a loaded Service target. */
export class ServiceCompletionReporter extends RpcTarget {
  readonly #env: BindingEnv;
  readonly #rootFrame: () => string | null;

  constructor(env: BindingEnv, rootFrame: () => string | null) {
    super();
    this.#env = env;
    this.#rootFrame = rootFrame;
  }

  beginCapability(
    retention: string,
    frame: ServiceFrame,
  ): Promise<CapabilityAdmission> {
    if (!serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    return serviceControl(
      this.#env,
      "/internal/services/v1/capabilities/begin",
      {
        retention,
        parentFrame: frame.parentFrame ?? this.#rootFrame(),
      },
    );
  }

  releaseRetention(retention: string): Promise<unknown> {
    return serviceControl(this.#env, "/internal/services/v1/release", {
      handle: retention,
    });
  }

  completeOperation(handle: string): Promise<unknown> {
    return serviceControl(this.#env, "/internal/services/v1/complete", {
      handle,
    });
  }

  async retainCapability(
    handle: string,
    owner: "caller" | "target",
    deadlineAt: number,
  ): Promise<ServiceRetentionController> {
    if (!/^[0-9a-f-]{36}$/.test(handle))
      throw bindingError("SERVICE_BINDING_DENIED");
    return retainServiceCapability(
      this.#env,
      handle,
      owner,
      this.#rootFrame,
      deadlineAt,
    );
  }
}

/** Keep the owning private RPC context alive until its authority receipt arrives. */
export async function retainServiceCapability(
  env: BindingEnv,
  handle: string,
  owner: "caller" | "target",
  rootFrame: () => string | null,
  deadlineAt: number,
): Promise<ServiceRetentionController> {
  const remaining = deadlineAt - Date.now();
  if (!Number.isSafeInteger(deadlineAt) || remaining > 30_000)
    throw bindingError("SERVICE_UNAVAILABLE");
  if (remaining < 1) throw bindingError("SERVICE_TIMEOUT");
  const receipt = serviceControl<{ retention: string }>(
    env,
    "/internal/services/v1/retain",
    { handle, owner },
  ).then(async (retained) => {
    const controller = new ServiceRetentionController(
      env,
      retained.retention,
      rootFrame,
    );
    // Native RPC may cancel delivery after the caller's context ends. Revoke here first.
    if (Date.now() >= deadlineAt) {
      try {
        await controller.release();
      } catch {
        throw bindingError("SERVICE_UNAVAILABLE");
      }
      throw bindingError("SERVICE_TIMEOUT");
    }
    return controller;
  });
  // The original RPC context owns this authority hop even when its client disconnects.
  waitUntil(
    receipt.then(
      () => undefined,
      () => undefined,
    ),
  );
  return await receipt;
}

/** Own one capability retention; revocation leaves admitted operation cleanup available. */
export class ServiceRetentionController extends RpcTarget {
  readonly #env: BindingEnv;
  #retention: string | undefined;
  readonly #rootFrame: () => string | null;

  constructor(
    env: BindingEnv,
    retention: string,
    rootFrame: () => string | null,
  ) {
    super();
    this.#env = env;
    this.#retention = retention;
    this.#rootFrame = rootFrame;
  }

  begin(frame: ServiceFrame): Promise<CapabilityAdmission> {
    const retention = this.#retention;
    if (!retention || !serviceFrame(frame))
      throw bindingError("SERVICE_BINDING_DENIED");
    return serviceControl(
      this.#env,
      "/internal/services/v1/capabilities/begin",
      {
        retention,
        parentFrame: frame.parentFrame ?? this.#rootFrame(),
      },
    );
  }

  complete(handle: string): Promise<unknown> {
    if (!/^[0-9a-f-]{36}$/.test(handle)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    return serviceControl(this.#env, "/internal/services/v1/complete", {
      handle,
    });
  }

  async release(): Promise<void> {
    const retention = this.#retention;
    this.#retention = undefined;
    if (retention) {
      await serviceControl(this.#env, "/internal/services/v1/release", {
        handle: retention,
      });
    }
  }

  [Symbol.dispose](): void {
    waitUntil(this.release());
  }
}

/** Activate one admitted capability batch using the host authority backend. */
export async function activateCapabilities(
  env: BindingEnv,
  value: unknown,
  operationHandle: string,
  owner: "caller" | "target",
  deadlineAt: number,
): Promise<void> {
  await activateServiceCapabilities(
    value,
    () =>
      retainServiceCapability(
        env,
        operationHandle,
        owner,
        () => null,
        deadlineAt,
      ),
    deadlineAt,
    (source) => source,
  );
}
