import { RpcTarget, WorkerEntrypoint, waitUntil } from "cloudflare:workers";
import { routeDefaultHttp } from "../assets/router.js";
import type { BindingEnv, ServiceBindingProps } from "../bindings/protocol.js";
import { tenantEnv } from "../loader/bindings.js";
import { modulesFor } from "../loader/modules.js";
import type { LoaderEnv, RuntimeSnapshot } from "../loader/protocol.js";
import {
  inboundSocketTargetAddress,
  tunnelSockets,
} from "../sockets/tunnel.js";
import {
  assembleOnce, bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration,
  doPolicy, INTERNAL_HEADERS, lockWorkerCode, resolveSnapshot, tenantGlobalOutbound,
} from "../loader/shared.js";

interface ServiceFrame {
  readonly scopeId: string;
  readonly parentFrame: string | null;
}
interface ServiceAdmission {
  handle: string;
  frame: string;
  callerFrame: string;
  deadlineMs: number;
  target: {
    loaderKey: string;
    workerCodeSha256: string;
    routeGeneration: number;
    contentKind: "worker" | "assets_only";
    entrypoint?: string;
  };
}
interface CapabilityAdmission { handle: string; frame: string; deadlineMs: number }
interface CapabilityEnvelope {
  __openComputeServiceCapability: 1;
  kind: "function" | "target";
  handle: object;
}
interface ServiceRequestWire {
  url: string;
  method: string;
  headers: readonly (readonly [string, string])[];
  body: ReadableStream<Uint8Array> | null;
}
interface ServiceDispatchEnvelope {
  ok: boolean;
  value?: unknown;
  error?: unknown;
  background: ReadableStream<Uint8Array>;
}

const serviceRoots = new Map<string, { frame: string; expiresAt: number }>();
const SERVICE_METHOD = /^[A-Za-z_$][A-Za-z0-9_$]{0,127}$/;
const SERVICE_RESERVED = new Set([
  "constructor", "prototype", "__proto__", "then", "dup",
  "__openComputeServiceRpc", "__openComputeServiceFetch", "__openComputeServiceGet",
]);

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function serviceObject(value: unknown): value is object {
  return value !== null && (typeof value === "object" || typeof value === "function");
}

function serviceCallable(value: unknown): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}

function serviceFrame(value: unknown): value is ServiceFrame {
  return record(value) && typeof value.scopeId === "string"
    && (value.parentFrame === null || typeof value.parentFrame === "string")
    && /^[0-9a-f-]{36}$/.test(value.scopeId)
    && (value.parentFrame === null || /^[0-9a-f-]{36}$/.test(value.parentFrame));
}

function serviceCapability(value: unknown): value is CapabilityEnvelope {
  return record(value) && value.__openComputeServiceCapability === 1
    && (value.kind === "function" || value.kind === "target")
    && serviceObject(value.handle);
}

async function serviceControl<T>(env: BindingEnv, path: string, body: unknown): Promise<T> {
  const response = await env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      [BINDING_TOKEN_HEADER]: env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
    },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw bindingError(response.headers.get("x-open-compute-error-code") || "SERVICE_UNAVAILABLE");
  }
  const value: unknown = await response.json();
  return value as T;
}

async function finalizeServiceConnect(env: BindingEnv, admission: ServiceAdmission): Promise<void> {
  let lastFailure: unknown;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      await serviceControl(env, "/internal/services/v1/connect/finalize", {
        handle: admission.handle,
        callerFrame: admission.callerFrame,
      });
      return;
    } catch (error) {
      lastFailure = error;
      if (attempt < 2) await scheduler.wait(10 * (attempt + 1));
    }
  }
  throw lastFailure;
}

class ServiceDrain {
  readonly #env: BindingEnv;
  readonly #handle: string;
  #background = false;
  #result = false;
  #completed = false;

  constructor(env: BindingEnv, handle: string) {
    this.#env = env;
    this.#handle = handle;
  }

  backgroundDone(): Promise<void> {
    this.#background = true;
    return this.#complete();
  }

  resultDone(): void {
    this.#result = true;
    waitUntil(this.#complete());
  }

  forceDone(): Promise<void> {
    this.#background = true;
    this.#result = true;
    return this.#complete();
  }

  async #complete(): Promise<void> {
    if (this.#completed || !this.#background || !this.#result) return;
    this.#completed = true;
    await serviceControl(this.#env, "/internal/services/v1/complete", { handle: this.#handle });
  }
}

function serviceDispatchEnvelope(value: unknown): value is ServiceDispatchEnvelope {
  return record(value) && typeof value.ok === "boolean"
    && value.background instanceof ReadableStream;
}

function startServiceBackground(
  envelope: ServiceDispatchEnvelope,
  drain: ServiceDrain,
): void {
  const reader = envelope.background.getReader();
  const completion = (async () => {
    try {
      for (;;) {
        const part = await reader.read();
        if (part.done) break;
      }
      await drain.backgroundDone();
    } finally {
      reader.releaseLock();
    }
  })();
  waitUntil(completion.catch(() => undefined));
}

function unwrapServiceDispatch(value: unknown, drain: ServiceDrain): unknown {
  if (!serviceDispatchEnvelope(value)) throw bindingError("SERVICE_UNAVAILABLE");
  startServiceBackground(value, drain);
  if (!value.ok) throw value.error;
  return value.value;
}

class ServiceCompletionReporter extends RpcTarget {
  readonly #env: BindingEnv;
  readonly #rootFrame: () => string | null;

  constructor(env: BindingEnv, rootFrame: () => string | null) {
    super();
    this.#env = env;
    this.#rootFrame = rootFrame;
  }

  beginCapability(retention: string, frame: ServiceFrame): Promise<CapabilityAdmission> {
    if (!serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    return serviceControl(this.#env, "/internal/services/v1/capabilities/begin", {
      retention,
      parentFrame: frame.parentFrame ?? this.#rootFrame(),
    });
  }

  releaseRetention(retention: string): Promise<unknown> {
    return serviceControl(this.#env, "/internal/services/v1/release", { handle: retention });
  }

  completeOperation(handle: string): Promise<unknown> {
    return serviceControl(this.#env, "/internal/services/v1/complete", { handle });
  }

  async retainCapability(
    handle: string,
    owner: "caller" | "target",
  ): Promise<ServiceRetentionController> {
    if (!/^[0-9a-f-]{36}$/.test(handle)) throw bindingError("SERVICE_BINDING_DENIED");
    const retained = await serviceControl<{ retention: string }>(
      this.#env,
      "/internal/services/v1/retain",
      { handle, owner },
    );
    return new ServiceRetentionController(this.#env, retained.retention, this.#rootFrame);
  }
}

class ServiceRetentionController extends RpcTarget {
  readonly #env: BindingEnv;
  #retention: string | undefined;
  readonly #rootFrame: () => string | null;

  constructor(env: BindingEnv, retention: string, rootFrame: () => string | null) {
    super();
    this.#env = env;
    this.#retention = retention;
    this.#rootFrame = rootFrame;
  }

  begin(frame: ServiceFrame): Promise<CapabilityAdmission> {
    const retention = this.#retention;
    if (!retention || !serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    return serviceControl(this.#env, "/internal/services/v1/capabilities/begin", {
      retention,
      parentFrame: frame.parentFrame ?? this.#rootFrame(),
    });
  }

  complete(handle: string): Promise<unknown> {
    if (!this.#retention || !/^[0-9a-f-]{36}$/.test(handle)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    return serviceControl(this.#env, "/internal/services/v1/complete", { handle });
  }

  async release(): Promise<void> {
    const retention = this.#retention;
    this.#retention = undefined;
    if (retention) {
      await serviceControl(this.#env, "/internal/services/v1/release", { handle: retention });
    }
  }
}

async function activateCapabilities(
  env: BindingEnv,
  value: unknown,
  operationHandle: string,
  owner: "caller" | "target",
  seen = new WeakSet<object>(),
): Promise<void> {
  if (!serviceObject(value) || seen.has(value)) return;
  seen.add(value);
  if (serviceCapability(value)) {
    const retained = await serviceControl<{ retention: string }>(
      env,
      "/internal/services/v1/retain",
      { handle: operationHandle, owner },
    );
    const controller = new ServiceRetentionController(env, retained.retention, () => null);
    const activate = Reflect.get(value.handle, "activate");
    if (!serviceCallable(activate)) {
      await controller.release();
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    try {
      await Reflect.apply(activate, value.handle, [controller]);
    } catch (error) {
      await controller.release();
      throw error;
    }
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value) await activateCapabilities(env, item, operationHandle, owner, seen);
    return;
  }
  if (Object.getPrototypeOf(value) !== Object.prototype && Object.getPrototypeOf(value) !== null) return;
  for (const item of Object.values(value)) {
    await activateCapabilities(env, item, operationHandle, owner, seen);
  }
}

function drainedStream(
  stream: ReadableStream<Uint8Array>,
  done: () => void,
): ReadableStream<Uint8Array> {
  const reader = stream.getReader();
  let finished = false;
  const finish = () => { if (!finished) { finished = true; done(); } };
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      try {
        const part = await reader.read();
        if (part.done) { finish(); controller.close(); }
        else controller.enqueue(part.value);
      } catch (error) { finish(); controller.error(error); }
    },
    async cancel(reason) { try { await reader.cancel(reason); } finally { finish(); } },
  });
}

function drainedWritable(stream: WritableStream<unknown>, done: () => void): WritableStream<unknown> {
  const writer = stream.getWriter();
  let finished = false;
  const finish = () => { if (!finished) { finished = true; done(); } };
  writer.closed.then(finish, finish);
  return new WritableStream<unknown>({
    write(chunk) { return writer.write(chunk); },
    async close() { try { await writer.close(); } finally { finish(); } },
    async abort(reason) { try { await writer.abort(reason); } finally { finish(); } },
  });
}

function trackServiceResult(value: unknown, drain: ServiceDrain): unknown {
  if (value instanceof Response) {
    if (value.webSocket) {
      value.webSocket.addEventListener("close", () => drain.resultDone(), { once: true });
      value.webSocket.addEventListener("error", () => drain.resultDone(), { once: true });
      return value;
    }
    if (!value.body) { drain.resultDone(); return value; }
    return new Response(drainedStream(value.body, () => drain.resultDone()), {
      status: value.status, statusText: value.statusText, headers: value.headers,
    });
  }
  if (value instanceof ReadableStream) {
    return drainedStream(value, () => drain.resultDone());
  }
  if (value instanceof WritableStream) {
    return drainedWritable(value, () => drain.resultDone());
  }
  if (value instanceof Request) {
    if (!value.body) { drain.resultDone(); return value; }
    return new Request(value, { body: drainedStream(value.body, () => drain.resultDone()) });
  }
  drain.resultDone();
  return value;
}

function serviceDeadlineAt(deadlineMs: number): number {
  if (!Number.isSafeInteger(deadlineMs) || deadlineMs < 1 || deadlineMs > 30_000) {
    throw bindingError("SERVICE_UNAVAILABLE");
  }
  return Date.now() + deadlineMs;
}

function serviceDeadline<T>(promise: Promise<T>, deadlineAt: number): Promise<T> {
  const remaining = deadlineAt - Date.now();
  if (remaining < 1) throw bindingError("SERVICE_TIMEOUT");
  return Promise.race([
    promise,
    scheduler.wait(remaining).then(() => { throw bindingError("SERVICE_TIMEOUT"); }),
  ]);
}

async function loadedServiceTarget(
  env: LoaderEnv,
  ctx: ExecutionContext,
  admission: ServiceAdmission,
): Promise<{ snapshot: RuntimeSnapshot; target: object }> {
  const envelope = {
    loaderKey: admission.target.loaderKey,
    expected: admission.target.workerCodeSha256,
  };
  const snapshot = await resolveSnapshot(
    env, envelope, false, Boolean(admission.target.entrypoint), env.INTERNAL_TOKEN,
  );
  if (snapshot.routeGeneration !== admission.target.routeGeneration
      || snapshot.contentKind !== admission.target.contentKind) {
    throw bindingError("DEPLOYMENT_INVARIANT_VIOLATION");
  }
  if (snapshot.contentKind !== "worker") throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
  const entrypoint = admission.target.entrypoint;
  const runtimeKey = `service/${admission.target.loaderKey}/${admission.target.workerCodeSha256}/g/${admission.target.routeGeneration}/${entrypoint || "default"}`;
  const code = await assembleOnce(runtimeKey, async () => {
    const built = modulesFor(snapshot, false, entrypoint);
    const deploymentId = admission.target.loaderKey.split("/")[2]!;
    return {
      ...lockWorkerCode(env),
      mainModule: built.mainModule,
      modules: built.modules,
      env: tenantEnv(
        snapshot, ctx, deploymentId, doPolicy(env), false, true, entrypoint ?? "default",
      ),
      globalOutbound: tenantGlobalOutbound(env, false),
    };
  });
  const stub = env.LOADER.get(runtimeKey, () => code);
  const runtimeEntrypoint = entrypoint ?? "__OpenComputeDefaultService";
  return {
    snapshot,
    target: stub.getEntrypoint(runtimeEntrypoint) as object,
  };
}

/** Generation-authenticated native Service Binding transport. */
export class ServiceTransport extends WorkerEntrypoint<LoaderEnv, ServiceBindingProps> {
  #props(): ServiceBindingProps {
    const props = this.ctx.props;
    if (!props || typeof props.deploymentId !== "string" || typeof props.bindingName !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    return props;
  }

  #parent(frame: ServiceFrame): string | null {
    if (!serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    if (frame.parentFrame) return frame.parentFrame;
    const root = serviceRoots.get(frame.scopeId);
    if (!root) return null;
    if (Date.now() >= root.expiresAt) {
      serviceRoots.delete(frame.scopeId);
      throw bindingError("SERVICE_TIMEOUT");
    }
    return root.frame;
  }

  async #admit(
    frame: ServiceFrame,
    operation: "default_fetch" | "named_fetch" | "rpc" | "connect",
  ): Promise<ServiceAdmission> {
    const props = this.#props();
    const parentFrame = this.#parent(frame);
    const admitted = await serviceControl<ServiceAdmission>(
      this.env,
      "/internal/services/v1/resolve",
      {
        callerDeploymentId: props.deploymentId,
        bindingName: props.bindingName,
        descriptorSha256: props.descriptorSha256,
        parentFrame,
        operation,
      },
    );
    if (!record(admitted) || typeof admitted.handle !== "string"
        || typeof admitted.frame !== "string" || typeof admitted.callerFrame !== "string"
        || !record(admitted.target)) throw bindingError("SERVICE_UNAVAILABLE");
    if (frame.parentFrame === null && parentFrame === null) {
      serviceRoots.set(frame.scopeId, {
        frame: admitted.callerFrame,
        expiresAt: Date.now() + admitted.deadlineMs,
      });
    }
    return admitted;
  }

  rpc(frame: ServiceFrame, method: string, args: unknown[]): Promise<unknown> {
    return this.#invoke(frame, method, args, false);
  }

  get(frame: ServiceFrame, property: string): Promise<unknown> {
    return this.#invoke(frame, property, [], true);
  }

  async connect(socket: Socket): Promise<void> {
    let admitted: ServiceAdmission | undefined;
    const frame = Object.freeze({ scopeId: crypto.randomUUID(), parentFrame: null });
    try {
      const address = await inboundSocketTargetAddress(socket);
      admitted = await this.#admit(frame, "connect");
      const loaded = await loadedServiceTarget(this.env, this.ctx, admitted);
      if (!serviceObject(loaded.target) || !serviceCallable(Reflect.get(loaded.target, "connect"))) {
        throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
      }
      const target = (loaded.target as Fetcher).connect(address, { allowHalfOpen: true });
      await target.opened;
      await tunnelSockets(socket, target);
    } catch {
      await socket.close().catch(() => undefined);
      throw bindingError("SERVICE_UNAVAILABLE");
    } finally {
      if (admitted) {
        try {
          await finalizeServiceConnect(this.env, admitted);
        } finally {
          serviceRoots.delete(frame.scopeId);
        }
      }
    }
  }

  async #invoke(
    frame: ServiceFrame,
    method: string,
    args: unknown[],
    getter: boolean,
  ): Promise<unknown> {
    if (!SERVICE_METHOD.test(method) || SERVICE_RESERVED.has(method) || !Array.isArray(args)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    const admitted = await this.#admit(frame, "rpc");
    const deadlineAt = serviceDeadlineAt(admitted.deadlineMs);
    const drain = new ServiceDrain(this.env, admitted.handle);
    const reporter = new ServiceCompletionReporter(
      this.env, () => serviceRoots.get(frame.scopeId)?.frame ?? null,
    );
    let dispatched = false;
    try {
      await activateCapabilities(this.env, args, admitted.handle, "caller");
      const loaded = await loadedServiceTarget(this.env, this.ctx, admitted);
      const call = Reflect.get(
        loaded.target,
        getter ? "__openComputeServiceGet" : "__openComputeServiceRpc",
      );
      if (!serviceCallable(call)) throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
      dispatched = true;
      const invocation = getter
        ? Reflect.apply(call, loaded.target, [frame.scopeId, admitted.frame, reporter, method])
        : Reflect.apply(call, loaded.target, [frame.scopeId, admitted.frame, reporter, method, args]);
      const dispatch = Promise.resolve(invocation).then((value) =>
        unwrapServiceDispatch(value, drain)
      );
      const value = await serviceDeadline(dispatch, deadlineAt);
      await activateCapabilities(this.env, value, admitted.handle, "target");
      return trackServiceResult(value, drain);
    } catch (error) {
      drain.resultDone();
      if (!dispatched) await drain.forceDone();
      throw error;
    }
  }

  async fetchService(frame: ServiceFrame, input: ServiceRequestWire): Promise<Response> {
    if (!serviceFrame(frame) || !record(input) || typeof input.url !== "string"
        || typeof input.method !== "string" || !Array.isArray(input.headers)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    const props = this.#props();
    const admitted = await this.#admit(frame, props.entrypoint ? "named_fetch" : "default_fetch");
    const deadlineAt = serviceDeadlineAt(admitted.deadlineMs);
    const drain = new ServiceDrain(this.env, admitted.handle);
    let dispatched = false;
    try {
      const headers = new Headers(input.headers);
      for (const name of INTERNAL_HEADERS) headers.delete(name);
      const init: RequestInit = {
        method: input.method, headers, body: input.body, redirect: "manual",
      };
      if (input.method === "GET" || input.method === "HEAD") delete init.body;
      const request = new Request(input.url, init);
      const envelope = {
        loaderKey: admitted.target.loaderKey,
        expected: admitted.target.workerCodeSha256,
      };
      const snapshot = await resolveSnapshot(
        this.env, envelope, false, Boolean(admitted.target.entrypoint), this.env.INTERNAL_TOKEN,
      );
      if (!admitted.target.entrypoint && routeDefaultHttp(snapshot, request) === "asset") {
        dispatched = true;
        drain.backgroundDone().catch(() => undefined);
        const deploymentId = admitted.target.loaderKey.split("/")[2]!;
        const response = await serviceDeadline(
          this.ctx.exports.AssetTransport({ props: Object.freeze({
            deploymentId,
            descriptorSha256: admitted.target.workerCodeSha256,
          }) }).fetch(request),
          deadlineAt,
        );
        // The Rust-owned asset body has its own precise deployment pin.
        await drain.forceDone();
        return response;
      }
      const loaded = await loadedServiceTarget(this.env, this.ctx, admitted);
      const call = Reflect.get(loaded.target, "__openComputeServiceFetch");
      if (!serviceCallable(call)) throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
      const reporter = new ServiceCompletionReporter(
        this.env, () => serviceRoots.get(frame.scopeId)?.frame ?? null,
      );
      dispatched = true;
      const dispatch = Promise.resolve(Reflect.apply(call, loaded.target, [
        frame.scopeId, admitted.frame, reporter, request,
      ])).then((value) => unwrapServiceDispatch(value, drain));
      const response = await serviceDeadline(dispatch, deadlineAt);
      if (!(response instanceof Response)) throw bindingError("SERVICE_UNAVAILABLE");
      return trackServiceResult(response, drain) as Response;
    } catch (error) {
      drain.resultDone();
      if (!dispatched) await drain.forceDone();
      throw error;
    }
  }

  beginCapability(retention: string, frame: ServiceFrame): Promise<CapabilityAdmission> {
    return serviceControl(this.env, "/internal/services/v1/capabilities/begin", {
      retention,
      parentFrame: this.#parent(frame),
    });
  }

  releaseRetention(retention: string): Promise<unknown> {
    return serviceControl(this.env, "/internal/services/v1/release", { handle: retention });
  }

  completeOperation(handle: string): Promise<unknown> {
    return serviceControl(this.env, "/internal/services/v1/complete", { handle });
  }

  async retainCapability(
    handle: string,
    owner: "caller" | "target",
  ): Promise<ServiceRetentionController> {
    if (!/^[0-9a-f-]{36}$/.test(handle)) throw bindingError("SERVICE_BINDING_DENIED");
    const retained = await serviceControl<{ retention: string }>(
      this.env,
      "/internal/services/v1/retain",
      { handle, owner },
    );
    return new ServiceRetentionController(this.env, retained.retention, () => null);
  }

  async completeRoot(scopeId: string): Promise<void> {
    if (!/^[0-9a-f-]{36}$/.test(scopeId)) throw bindingError("SERVICE_BINDING_DENIED");
    const root = serviceRoots.get(scopeId);
    if (!root) return;
    await serviceControl(this.env, "/internal/services/v1/root/complete", { frame: root.frame });
    serviceRoots.delete(scopeId);
  }
}
