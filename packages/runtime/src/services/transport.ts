import { RpcTarget, waitUntil, WorkerEntrypoint } from "cloudflare:workers";
import { routeDefaultHttp } from "../assets/router.js";
import type { BindingEnv, ServiceBindingProps } from "../bindings/protocol.js";
import { tenantEnv } from "../loader/bindings.js";
import { modulesFor } from "../loader/modules.js";
import { PRIVATE_POLICY, type WorkerPolicy } from "../loader/policy.js";
import type {
  LoaderEnv,
  NativeHostExtensionPort,
  RuntimeSnapshot,
} from "../loader/protocol.js";
import {
  assembleOnce,
  BINDING_TOKEN_HEADER,
  bindingError,
  currentStartupGeneration,
  doPolicy,
  INTERNAL_HEADERS,
  resolveSnapshot,
  snapshotWorkerCode,
  tenantGlobalOutbound,
} from "../loader/shared.js";
import { observedEntrypoint } from "../observability/collector.js";
import {
  inboundSocketTargetAddress,
  tunnelSockets,
} from "../sockets/tunnel.js";
import {
  activateCapabilities,
  retainServiceCapability,
  retryServiceControl,
  ServiceCompletionReporter,
  serviceControl,
  serviceFrame,
  type CapabilityAdmission,
  type ServiceRetentionController,
} from "./control.js";
import { serviceDeadline, serviceDeadlineAt } from "./deadline.js";
import { appendServiceWebSocketHandoff } from "./facade.js";
import type { ServiceFrame } from "./scope.js";

interface ServiceAdmission {
  handle: string;
  frame: string;
  callerFrame: string;
  deadlineMs: number;
  target:
    | {
        kind: "worker";
        loaderKey: string;
        workerCodeSha256: string;
        routeGeneration: number;
        contentKind: "worker" | "assets_only";
        entrypoint?: string;
        props?: Record<string, unknown>;
      }
    | {
        kind: "extension";
        loaderKey: string;
        mainModule: string;
        moduleBase64: string;
        sessionIdentity: string;
        entrypoint?: string;
        props?: Record<string, unknown>;
      }
    | {
        kind: "private_http";
        sessionIdentity: string;
      };
}
interface ServiceDispatchEnvelope {
  ok: boolean;
  value?: unknown;
  error?: unknown;
  background: ReadableStream<Uint8Array>;
}

interface ServiceRoot {
  frame: string | null;
  expiresAt: number;
  pending: Promise<void>;
  closing: boolean;
  completion?: Promise<void>;
}
const serviceRoots = new Map<string, ServiceRoot>();

/** Keep root cleanup in the host when the caller's native execution context ends. */
class ServiceRootLease extends RpcTarget {
  readonly #transport: ServiceTransport;
  readonly #scopeId: string;
  readonly #released: () => void;
  #closed = false;
  #pending = 0;

  constructor(
    transport: ServiceTransport,
    scopeId: string,
    released: () => void,
  ) {
    super();
    this.#transport = transport;
    this.#scopeId = scopeId;
    this.#released = released;
  }

  #check(frame: ServiceFrame): void {
    if (
      this.#closed ||
      !serviceFrame(frame) ||
      frame.scopeId !== this.#scopeId ||
      frame.parentFrame !== null
    )
      throw bindingError("SERVICE_BINDING_DENIED");
  }

  async #run<T>(action: () => Promise<T>): Promise<T> {
    this.#pending++;
    try {
      return await action();
    } finally {
      try {
        // A canceled caller may leave an admission already in flight on the host.
        if (this.#closed) await this.#transport.completeRoot(this.#scopeId);
      } finally {
        this.#pending--;
        this.#finish();
      }
    }
  }

  #finish(): void {
    if (this.#closed && this.#pending === 0) this.#released();
  }

  rpc(frame: ServiceFrame, method: string, args: unknown[]): Promise<unknown> {
    this.#check(frame);
    return this.#run(() => this.#transport.rpc(frame, method, args));
  }

  get(frame: ServiceFrame, property: string): Promise<unknown> {
    this.#check(frame);
    return this.#run(() => this.#transport.get(frame, property));
  }

  ready(): void {
    if (this.#closed) throw bindingError("SERVICE_BINDING_DENIED");
  }

  [Symbol.dispose](): void {
    if (this.#closed) return;
    this.#closed = true;
    waitUntil(
      this.#transport.completeRoot(this.#scopeId).finally(() => this.#finish()),
    );
  }
}
const SERVICE_RESERVED = new Set([
  "constructor",
  "__proto__",
  "then",
  "dup",
  "__openComputeServiceRpc",
  "__openComputeServiceFetch",
  "__openComputeServiceGet",
]);

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function serviceObject(value: unknown): value is object {
  return (
    value !== null && (typeof value === "object" || typeof value === "function")
  );
}

function serviceCallable(
  value: unknown,
): value is (...args: unknown[]) => unknown {
  return typeof value === "function";
}

async function finalizeServiceConnect(
  env: BindingEnv,
  admission: ServiceAdmission,
): Promise<void> {
  await retryServiceControl(env, "/internal/services/v1/connect/finalize", {
    handle: admission.handle,
    callerFrame: admission.callerFrame,
  });
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
    await retryServiceControl(this.#env, "/internal/services/v1/complete", {
      handle: this.#handle,
    });
  }
}

function serviceDispatchEnvelope(
  value: unknown,
): value is ServiceDispatchEnvelope {
  return (
    record(value) &&
    typeof value.ok === "boolean" &&
    value.background instanceof ReadableStream
  );
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

async function unwrapServiceDispatch(
  value: unknown,
  drain: ServiceDrain,
): Promise<unknown> {
  try {
    if (!serviceDispatchEnvelope(value))
      throw bindingError("SERVICE_UNAVAILABLE");
    startServiceBackground(value, drain);
  } catch {
    await drain.forceDone();
    throw bindingError("SERVICE_UNAVAILABLE");
  }
  if (!value.ok) throw value.error;
  return value.value;
}

function drainedStream(
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

function drainedWritable(
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

function trackServiceResult(value: unknown, drain: ServiceDrain): unknown {
  if (value instanceof Response) {
    if (value.webSocket) {
      value.webSocket.addEventListener("close", () => drain.resultDone(), {
        once: true,
      });
      value.webSocket.addEventListener("error", () => drain.resultDone(), {
        once: true,
      });
      return value;
    }
    if (!value.body) {
      drain.resultDone();
      return value;
    }
    return new Response(
      drainedStream(value.body, () => drain.resultDone()),
      {
        status: value.status,
        statusText: value.statusText,
        headers: value.headers,
      },
    );
  }
  if (value instanceof ReadableStream) {
    return drainedStream(value, () => drain.resultDone());
  }
  if (value instanceof WritableStream) {
    return drainedWritable(value, () => drain.resultDone());
  }
  if (value instanceof Request) {
    if (!value.body) {
      drain.resultDone();
      return value;
    }
    return new Request(value, {
      body: drainedStream(value.body, () => drain.resultDone()),
    });
  }
  drain.resultDone();
  return value;
}

function extensionEnvironment(
  host: NativeHostExtensionPort,
  entrypoint: string | undefined,
  cache: object,
  policy: WorkerPolicy,
) {
  return {
    env: { HOST: host },
    openComputePrivateEnv: {
      [PRIVATE_POLICY]: policy,
      __OPEN_COMPUTE_PRIVATE_CACHE: Object.freeze({
        [entrypoint ?? "default"]: cache,
      }),
    },
    openComputeHostPolicy: true as const,
  };
}

async function loadedServiceTarget(
  env: LoaderEnv,
  ctx: ExecutionContext,
  admission: ServiceAdmission,
  serviceContext?: { scopeId: string; frame: string; completion?: Fetcher },
): Promise<{ snapshot: RuntimeSnapshot; target: Fetcher }> {
  if (admission.target.kind === "private_http") {
    return {
      snapshot: {
        schemaVersion: 1,
        loaderKey: `private-http/${admission.target.sessionIdentity}`,
        workerCodeSha256: "private-http",
        routeGeneration: 1,
        compatibilityDate: env.SYSTEM_COMPATIBILITY_DATE,
        compatibilityFlags: [...env.SYSTEM_COMPATIBILITY_FLAGS],
        limits: { cpuMs: 30_000, subRequests: 1_000 },
        contentKind: "worker",
        mainModule: "private-http",
        modules: [],
        moduleBindings: [],
        workerLoaders: [],
        browserBindings: [],
        env: {},
        bindings: [],
        scheduledTargets: [],
        services: [],
        cachePolicy: {
          enabled: false,
          crossVersionCache: false,
          failOpen: false,
          entrypoints: {},
        },
      },
      target: ctx.exports.PrivateHttpTransport({
        props: { sessionIdentity: admission.target.sessionIdentity },
      }),
    };
  }
  if (admission.target.kind === "extension") {
    const extension = admission.target;
    const source = atob(extension.moduleBase64);
    const bytes = new Uint8Array(source.length);
    for (let index = 0; index < source.length; index += 1)
      bytes[index] = source.charCodeAt(index);
    const snapshot: RuntimeSnapshot = {
      schemaVersion: 1,
      loaderKey: extension.loaderKey,
      workerCodeSha256: "extension",
      routeGeneration: 1,
      compatibilityDate: env.SYSTEM_COMPATIBILITY_DATE,
      compatibilityFlags: [...env.SYSTEM_COMPATIBILITY_FLAGS],
      limits: { cpuMs: 30_000, subRequests: 1_000 },
      contentKind: "worker",
      mainModule: extension.mainModule,
      modules: [
        {
          name: extension.mainModule,
          type: "esModule",
          bytesBase64: extension.moduleBase64,
        },
      ],
      moduleBindings: [],
      workerLoaders: [],
      browserBindings: [],
      env: {},
      bindings: [],
      scheduledTargets: [],
      services: [],
      cachePolicy: {
        enabled: false,
        crossVersionCache: false,
        failOpen: false,
        entrypoints: {},
      },
    };
    new TextDecoder("utf-8", { fatal: true, ignoreBOM: false }).decode(bytes);
    const entrypoint = extension.entrypoint;
    const stub = env.LOADER.get(extension.loaderKey, async () => {
      const built = modulesFor(snapshot, false, entrypoint);
      return {
        ...(await snapshotWorkerCode(
          env,
          snapshot,
          "runtime",
          env.INTERNAL_TOKEN,
        )),
        mainModule: built.mainModule,
        modules: built.modules,
        ...extensionEnvironment(
          env.HOST_EXTENSION_FACTORY.get(extension.sessionIdentity),
          entrypoint,
          ctx.exports.ExtensionCacheTransport({ props: {} }),
          built.policy,
        ),
        globalOutbound: null,
      };
    });
    return {
      snapshot,
      target: env.WORKER_LOADER_FACTORY.getEntrypoint(
        stub,
        [],
        entrypoint ?? "__OpenComputeDefaultService",
        serviceContext === undefined
          ? extension.props === undefined
            ? undefined
            : { props: extension.props }
          : {
              props: {
                __OPEN_COMPUTE_SERVICE_CONTEXT: serviceContext,
                userProps: extension.props,
              },
            },
      ),
    };
  }
  const worker = admission.target;
  const envelope = {
    loaderKey: worker.loaderKey,
    expected: worker.workerCodeSha256,
    routeGeneration: worker.routeGeneration,
  };
  const snapshot = await resolveSnapshot(
    env,
    envelope,
    "runtime",
    env.INTERNAL_TOKEN,
  );
  if (
    snapshot.routeGeneration !== worker.routeGeneration ||
    snapshot.contentKind !== worker.contentKind
  ) {
    throw bindingError("VERSION_INVARIANT_VIOLATION");
  }
  if (snapshot.contentKind !== "worker")
    throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
  const entrypoint = worker.entrypoint;
  const runtimeKey =
    `service/${worker.loaderKey}/${worker.workerCodeSha256}` +
    `/${entrypoint || "default"}`;
  const stub = env.LOADER.get(runtimeKey, async () => {
    const code = await assembleOnce(runtimeKey, async () => {
      const built = modulesFor(snapshot, false, entrypoint);
      const versionId = worker.loaderKey.split("/")[2]!;
      return {
        ...(await snapshotWorkerCode(
          env,
          snapshot,
          "runtime",
          env.INTERNAL_TOKEN,
        )),
        mainModule: built.mainModule,
        modules: built.modules,
        ...tenantEnv(
          snapshot,
          built.policy,
          ctx,
          env.WORKER_LOADER_FACTORY,
          versionId,
          doPolicy(env),
          false,
          entrypoint ?? "default",
        ),
        globalOutbound: tenantGlobalOutbound(env, false),
      };
    });
    return code;
  });
  const runtimeEntrypoint = entrypoint ?? "__OpenComputeDefaultService";
  return {
    snapshot,
    target: observedEntrypoint(
      stub,
      env.WORKER_LOADER_FACTORY,
      ctx,
      snapshot.observability,
      runtimeEntrypoint,
      serviceContext === undefined
        ? worker.props === undefined
          ? undefined
          : { props: worker.props }
        : {
            props: {
              __OPEN_COMPUTE_SERVICE_CONTEXT: serviceContext,
              userProps: worker.props,
            },
          },
    ),
  };
}

/** Completion capability is created by the admitted transport and hidden from tenant props. */
export class ServiceFetchCompletion extends WorkerEntrypoint<
  LoaderEnv,
  { handle: string }
> {
  async fetch(): Promise<Response> {
    await retryServiceControl(this.env, "/internal/services/v1/complete", {
      handle: this.ctx.props.handle,
    });
    return new Response(null, { status: 204 });
  }
}

/** Serializable deny-all Cache transport used by host-extension facades. */
export class ExtensionCacheTransport extends WorkerEntrypoint {
  match(): never {
    throw bindingError("CACHE_UNAVAILABLE");
  }

  put(): never {
    throw bindingError("CACHE_UNAVAILABLE");
  }

  delete(): never {
    throw bindingError("CACHE_UNAVAILABLE");
  }

  purge(): never {
    throw bindingError("CACHE_UNAVAILABLE");
  }
}

export class PrivateHttpTransport extends WorkerEntrypoint<
  LoaderEnv,
  { sessionIdentity: string }
> {
  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const headers = new Headers(request.headers);
    headers.delete("authorization");
    headers.delete("cookie");
    headers.delete("proxy-authorization");
    headers.set(
      "x-open-compute-private-service",
      this.ctx.props.sessionIdentity,
    );
    headers.set("x-open-compute-private-path", `${url.pathname}${url.search}`);
    headers.set(BINDING_TOKEN_HEADER, this.env.BINDING_BACKEND_TOKEN);
    headers.set(
      "x-open-compute-startup-generation",
      currentStartupGeneration(),
    );
    return this.env.BINDING_BACKEND.fetch(
      "http://binding-backend/internal/services/v1/private-http",
      {
        method: request.method,
        headers,
        body: request.body,
      },
    );
  }
}

/** Generation-authenticated native Service Binding transport. */
export class ServiceTransport extends WorkerEntrypoint<
  LoaderEnv,
  ServiceBindingProps
> {
  root(scopeId: string): ServiceRootLease {
    this.#props();
    if (!/^[0-9a-f-]{36}$/.test(scopeId))
      throw bindingError("SERVICE_BINDING_DENIED");
    const released = Promise.withResolvers<void>();
    // RPC disconnect cancels a callee context unless its cleanup owns waitUntil work.
    this.ctx.waitUntil(released.promise);
    return new ServiceRootLease(this, scopeId, released.resolve);
  }

  #props(): ServiceBindingProps {
    const props = this.ctx.props;
    if (
      !props ||
      typeof props.versionId !== "string" ||
      typeof props.bindingName !== "string" ||
      !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
    ) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    return props;
  }

  async #parent(frame: ServiceFrame): Promise<string | null> {
    if (!serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    if (frame.parentFrame) return frame.parentFrame;
    const root = serviceRoots.get(frame.scopeId);
    if (!root) return null;
    await root.pending;
    if (root.closing || root.frame === null)
      throw bindingError("SERVICE_BINDING_DENIED");
    // Keep the deadline fence until event completion; absence would admit a fresh root.
    if (Date.now() >= root.expiresAt) throw bindingError("SERVICE_TIMEOUT");
    return root.frame;
  }

  async #admit(
    frame: ServiceFrame,
    operation: "default_fetch" | "named_fetch" | "rpc" | "connect",
  ): Promise<ServiceAdmission> {
    const props = this.#props();
    if (!serviceFrame(frame)) throw bindingError("SERVICE_BINDING_DENIED");
    let root =
      frame.parentFrame === null ? serviceRoots.get(frame.scopeId) : undefined;
    let first: ReturnType<typeof Promise.withResolvers<void>> | undefined;
    if (frame.parentFrame === null && !root) {
      first = Promise.withResolvers<void>();
      root = {
        frame: null,
        expiresAt: 0,
        pending: first.promise,
        closing: false,
      };
      // Publish before awaiting the private hop so siblings cannot create a second root.
      serviceRoots.set(frame.scopeId, root);
    }
    try {
      const parentFrame = first ? null : await this.#parent(frame);
      const admitted = await serviceControl<ServiceAdmission>(
        this.env,
        "/internal/services/v1/resolve",
        {
          callerVersionId: props.versionId,
          bindingName: props.bindingName,
          descriptorSha256: props.descriptorSha256,
          parentFrame,
          operation,
        },
      );
      if (
        !record(admitted) ||
        typeof admitted.handle !== "string" ||
        typeof admitted.frame !== "string" ||
        typeof admitted.callerFrame !== "string" ||
        !record(admitted.target)
      )
        throw bindingError("SERVICE_UNAVAILABLE");
      if (first && root) {
        root.frame = admitted.callerFrame;
        root.expiresAt = Date.now() + admitted.deadlineMs;
      }
      if (root?.closing) {
        await retryServiceControl(this.env, "/internal/services/v1/complete", {
          handle: admitted.handle,
        });
        throw bindingError("SERVICE_BINDING_DENIED");
      }
      return admitted;
    } finally {
      first?.resolve();
    }
  }

  rpc(frame: ServiceFrame, method: string, args: unknown[]): Promise<unknown> {
    return this.#invoke(frame, method, args, false);
  }

  get(frame: ServiceFrame, property: string): Promise<unknown> {
    return this.#invoke(frame, property, [], true);
  }

  connect(socket: Socket): Promise<void> {
    const completion = this.#connect(socket);
    // Native CONNECT cancellation must not discard the registry finalization request.
    this.ctx.waitUntil(completion);
    return completion;
  }

  async #connect(socket: Socket): Promise<void> {
    let admitted: ServiceAdmission | undefined;
    let target: Socket | undefined;
    const frame = Object.freeze({
      scopeId: crypto.randomUUID(),
      parentFrame: null,
    });
    try {
      const address = await inboundSocketTargetAddress(socket);
      const admission = await this.#admit(frame, "connect");
      admitted = admission;
      const deadlineAt = serviceDeadlineAt(admission.deadlineMs);
      const loaded = await serviceDeadline(
        () =>
          loadedServiceTarget(this.env, this.ctx, admission, {
            scopeId: frame.scopeId,
            frame: admission.frame,
          }),
        deadlineAt,
      );
      if (
        !serviceObject(loaded.target) ||
        !serviceCallable(Reflect.get(loaded.target, "connect"))
      ) {
        throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
      }
      const connected = await serviceDeadline(async () => {
        const opened = (loaded.target as Fetcher).connect(address, {
          allowHalfOpen: true,
        });
        target = opened;
        await opened.opened;
        return opened;
      }, deadlineAt);
      // The startup deadline does not limit an established TCP tunnel's lifetime.
      await tunnelSockets(socket, connected);
    } catch {
      await socket.close().catch(() => undefined);
      await target?.close().catch(() => undefined);
      throw bindingError("SERVICE_UNAVAILABLE");
    } finally {
      if (admitted) {
        try {
          await finalizeServiceConnect(this.env, admitted);
        } finally {
          serviceRoots.delete(frame.scopeId);
        }
      } else await this.completeRoot(frame.scopeId);
    }
  }

  async #invoke(
    frame: ServiceFrame,
    method: string,
    args: unknown[],
    getter: boolean,
  ): Promise<unknown> {
    if (
      typeof method !== "string" ||
      SERVICE_RESERVED.has(method) ||
      !Array.isArray(args)
    ) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    const admitted = await this.#admit(frame, "rpc");
    const drain = new ServiceDrain(this.env, admitted.handle);
    const reporter = new ServiceCompletionReporter(
      this.env,
      () => serviceRoots.get(frame.scopeId)?.frame ?? null,
    );
    let dispatched = false;
    try {
      const deadlineAt = serviceDeadlineAt(admitted.deadlineMs);
      await activateCapabilities(
        this.env,
        args,
        admitted.handle,
        "caller",
        deadlineAt,
      );
      const loaded = await serviceDeadline(
        () =>
          loadedServiceTarget(this.env, this.ctx, admitted, {
            scopeId: frame.scopeId,
            frame: admitted.frame,
          }),
        deadlineAt,
      );
      const call = Reflect.get(
        loaded.target,
        getter ? "__openComputeServiceGet" : "__openComputeServiceRpc",
      );
      if (!serviceCallable(call))
        throw bindingError("SERVICE_ENTRYPOINT_NOT_FOUND");
      const value = await serviceDeadline(() => {
        const invocation = getter
          ? Reflect.apply(call, loaded.target, [
              frame.scopeId,
              admitted.frame,
              reporter,
              method,
            ])
          : Reflect.apply(call, loaded.target, [
              frame.scopeId,
              admitted.frame,
              reporter,
              method,
              args,
            ]);
        dispatched = true;
        const dispatch = Promise.resolve(invocation).then(
          (value) => unwrapServiceDispatch(value, drain),
          async (error: unknown) => {
            await drain.forceDone();
            throw error;
          },
        );
        return dispatch;
      }, deadlineAt);
      await activateCapabilities(
        this.env,
        value,
        admitted.handle,
        "target",
        deadlineAt,
      );
      return trackServiceResult(value, drain);
    } catch (error) {
      drain.resultDone();
      if (!dispatched) await drain.forceDone();
      throw error;
    }
  }

  override fetch(request: Request): Promise<Response> {
    const completion = this.#fetch(request);
    // As with CONNECT, caller cancellation must not discard in-flight admission cleanup.
    this.ctx.waitUntil(completion);
    return completion;
  }

  async #fetch(request: Request): Promise<Response> {
    const raw: unknown = JSON.parse(
      request.headers.get("x-open-compute-service-frame") ?? "null",
    );
    if (!serviceFrame(raw)) {
      throw bindingError("SERVICE_BINDING_DENIED");
    }
    const props = this.#props();
    const admitted = await this.#admit(
      raw,
      props.entrypoint ? "named_fetch" : "default_fetch",
    );
    let dispatched = false;
    try {
      const deadlineAt = serviceDeadlineAt(admitted.deadlineMs);
      const headers = new Headers(request.headers);
      for (const name of INTERNAL_HEADERS) headers.delete(name);
      request = new Request(request, { headers });
      const target = admitted.target;
      const snapshot =
        target.kind === "worker"
          ? await serviceDeadline(
              () =>
                resolveSnapshot(
                  this.env,
                  {
                    loaderKey: target.loaderKey,
                    expected: target.workerCodeSha256,
                    routeGeneration: target.routeGeneration,
                  },
                  "runtime",
                  this.env.INTERNAL_TOKEN,
                ),
              deadlineAt,
            )
          : undefined;
      if (
        target.kind === "worker" &&
        snapshot !== undefined &&
        !target.entrypoint &&
        routeDefaultHttp(snapshot, request) === "asset"
      ) {
        try {
          return await serviceDeadline(
            () =>
              this.ctx.exports
                .AssetTransport({
                  props: Object.freeze({
                    versionId: target.loaderKey.split("/")[2]!,
                    descriptorSha256: target.workerCodeSha256,
                  }),
                })
                .fetch(request),
            deadlineAt,
          );
        } finally {
          // The Rust-owned asset body retains its own version pin.
          dispatched = true;
          await retryServiceControl(
            this.env,
            "/internal/services/v1/complete",
            { handle: admitted.handle },
          );
        }
      }
      const completion = this.ctx.exports.ServiceFetchCompletion({
        props: { handle: admitted.handle },
      });
      const loaded = await serviceDeadline(
        () =>
          loadedServiceTarget(this.env, this.ctx, admitted, {
            scopeId: raw.scopeId,
            frame: admitted.frame,
            completion,
          }),
        deadlineAt,
      );
      const response = await serviceDeadline(() => {
        dispatched = true;
        return loaded.target.fetch(request);
      }, deadlineAt);
      if (!response.webSocket) return response;
      try {
        return appendServiceWebSocketHandoff(response, admitted.handle);
      } catch (error) {
        await retryServiceControl(this.env, "/internal/services/v1/complete", {
          handle: admitted.handle,
        });
        throw error;
      }
    } catch (error) {
      if (!dispatched)
        await retryServiceControl(this.env, "/internal/services/v1/complete", {
          handle: admitted.handle,
        });
      throw error;
    }
  }

  async beginCapability(
    retention: string,
    frame: ServiceFrame,
  ): Promise<CapabilityAdmission> {
    return serviceControl(
      this.env,
      "/internal/services/v1/capabilities/begin",
      {
        retention,
        parentFrame: await this.#parent(frame),
      },
    );
  }

  releaseRetention(retention: string): Promise<unknown> {
    return serviceControl(this.env, "/internal/services/v1/release", {
      handle: retention,
    });
  }

  completeOperation(handle: string): Promise<unknown> {
    return serviceControl(this.env, "/internal/services/v1/complete", {
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
      this.env,
      handle,
      owner,
      () => null,
      deadlineAt,
    );
  }

  async completeRoot(scopeId: string): Promise<void> {
    if (!/^[0-9a-f-]{36}$/.test(scopeId))
      throw bindingError("SERVICE_BINDING_DENIED");
    const root = serviceRoots.get(scopeId);
    if (!root) return;
    root.closing = true;
    root.completion ??= (async () => {
      try {
        await root.pending;
        if (root.frame !== null)
          await retryServiceControl(
            this.env,
            "/internal/services/v1/root/complete",
            {
              frame: root.frame,
            },
          );
        serviceRoots.delete(scopeId);
      } finally {
        delete root.completion;
      }
    })();
    await root.completion;
  }
}
