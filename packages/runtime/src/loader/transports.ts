import { RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import type {
  AssetBindingProps,
  BindingEnv,
  ResourceBindingProps,
} from "../bindings/protocol.js";
import { makeD1TransportBase } from "../d1/transport.js";
import type {
  AlarmIdentity,
  AlarmProjection,
} from "../durable-objects/protocol.js";
import { collectObservabilityTail } from "../observability/collector.js";
import type { QueueBindingProps } from "../queues/protocol.js";
import { makeR2TransportBase } from "../r2/transport.js";
import {
  inboundSocketAddress,
  tunnelSockets,
  validateSocketAuthorityWire,
  type SocketAuthorityWire,
} from "../sockets/tunnel.js";
import type { LoaderEnv, RuntimeObservabilityIdentity } from "./protocol.js";
import {
  BINDING_TOKEN_HEADER,
  bindingError,
  currentStartupGeneration,
  INTERNAL_HEADERS,
  isRecord,
} from "./shared.js";

export { ArtifactsTransport } from "../artifacts/transport.js";

/** Direct main-module entrypoint used by Worker Loader tail service stubs. */
export class ObservabilityTail extends WorkerEntrypoint<
  LoaderEnv,
  RuntimeObservabilityIdentity
> {
  async tail(events: TraceItem[]): Promise<void> {
    await collectObservabilityTail(events, this.env, this.ctx.props);
  }
}

const DO_ORDER_CHANNEL = /^[0-9a-f]{32}$/;
const doConnects = new Map<
  string,
  {
    authority: SocketAuthorityWire;
    channelId: string;
    expiresAt: number;
    objectId: string;
    sequence: number;
  }
>();
type PendingDoConnect = NonNullable<ReturnType<typeof doConnects.get>>;
const doConnectWaiters = new Map<
  string,
  {
    resolve: (pending: PendingDoConnect | undefined) => void;
    promise: Promise<PendingDoConnect | undefined>;
  }
>();

async function waitForDoConnect(
  operationId: string,
): Promise<PendingDoConnect | undefined> {
  const existing = doConnects.get(operationId);
  if (existing) return existing;
  if (doConnectWaiters.size >= 1024) throw bindingError("DO_STORAGE_LIMIT");
  let resolve: (pending: PendingDoConnect | undefined) => void = () => {};
  const promise = new Promise<PendingDoConnect | undefined>((done) => {
    resolve = done;
  });
  const waiter = { resolve, promise };
  doConnectWaiters.set(operationId, waiter);
  try {
    return await Promise.race([
      promise,
      scheduler.wait(10_000).then(() => undefined),
    ]);
  } finally {
    if (doConnectWaiters.get(operationId) === waiter)
      doConnectWaiters.delete(operationId);
  }
}

const R2TransportBase = makeR2TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class R2Transport extends R2TransportBase {}

const D1TransportBase = makeD1TransportBase(
  bindingError,
  currentStartupGeneration,
  BINDING_TOKEN_HEADER,
);

export class D1Transport extends D1TransportBase {}

export class QueueTransport extends WorkerEntrypoint<
  BindingEnv,
  QueueBindingProps
> {
  #props() {
    const props = this.ctx.props;
    if (
      !props ||
      typeof props.bindingId !== "string" ||
      typeof props.versionId !== "string" ||
      typeof props.queueId !== "string" ||
      !/^[0-9a-f]{64}$/.test(props.descriptorSha256) ||
      !Number.isSafeInteger(props.queueLifecycleGeneration) ||
      props.queueLifecycleGeneration < 1
    ) {
      throw bindingError("QUEUE_INVARIANT_VIOLATION");
    }
    return props;
  }

  async #request(
    operation: string,
    body?: BodyInit,
    operationId?: string,
  ): Promise<unknown> {
    const props = this.#props();
    if (
      operationId !== undefined &&
      (typeof operationId !== "string" ||
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(
          operationId,
        ))
    ) {
      throw bindingError("QUEUE_INVARIANT_VIOLATION");
    }
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/queue/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type":
            body === undefined
              ? "application/json"
              : "application/vnd.open-compute.queue.v1+frame",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-version-id": props.versionId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-request-id": operationId ?? crypto.randomUUID(),
          "x-open-compute-output-gate": operationId === undefined ? "0" : "1",
        },
        ...(body === undefined ? {} : { body }),
      },
    );
    if (!response.ok) {
      const code =
        response.headers.get("x-open-compute-error-code") ||
        "QUEUE_STORAGE_UNAVAILABLE";
      try {
        await response.body?.cancel();
      } catch {
        /* best effort */
      }
      throw bindingError(code);
    }
    const result: unknown = await response.json();
    if (!result || typeof result !== "object")
      throw bindingError("QUEUE_INVARIANT_VIOLATION");
    return result;
  }

  send(frame: Uint8Array, operationId?: string) {
    return this.#request("send", frame, operationId);
  }

  sendBatch(frame: Uint8Array, operationId?: string) {
    return this.#request("batch", frame, operationId);
  }

  async finalize(operationId: string): Promise<void> {
    await this.#request("finalize", undefined, operationId);
  }

  metrics() {
    return this.#request("metrics");
  }
}

export class AssetTransport extends WorkerEntrypoint<
  BindingEnv,
  AssetBindingProps
> {
  #props() {
    const props = this.ctx.props;
    if (
      !props ||
      typeof props.versionId !== "string" ||
      typeof props.descriptorSha256 !== "string" ||
      !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
    ) {
      throw bindingError("VERSION_INVARIANT_VIOLATION");
    }
    return props;
  }

  async fetchAsset(input: unknown): Promise<Response> {
    if (
      !isRecord(input) ||
      typeof input.url !== "string" ||
      typeof input.method !== "string" ||
      !Array.isArray(input.headers) ||
      input.headers.length > 256
    ) {
      throw bindingError("BINDING_PROTOCOL_ERROR");
    }
    const headers = new Headers();
    for (const pair of input.headers) {
      if (
        !Array.isArray(pair) ||
        pair.length !== 2 ||
        typeof pair[0] !== "string" ||
        typeof pair[1] !== "string"
      ) {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      headers.append(pair[0], pair[1]);
    }
    return this.#fetch(
      new Request(input.url, { method: input.method, headers }),
    );
  }

  fetch(request: Request): Promise<Response> {
    return this.#fetch(request);
  }

  async #fetch(request: Request): Promise<Response> {
    const props = this.#props();
    const headers = new Headers(request.headers);
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    headers.set(BINDING_TOKEN_HEADER, this.env.BINDING_BACKEND_TOKEN);
    headers.set(
      "x-open-compute-startup-generation",
      currentStartupGeneration(),
    );
    headers.set("x-open-compute-version-id", props.versionId);
    headers.set("x-open-compute-descriptor-sha256", props.descriptorSha256);
    headers.set("x-open-compute-request-id", crypto.randomUUID());
    headers.set("x-open-compute-asset-method", request.method);
    headers.set("x-open-compute-asset-url", request.url);
    const response = await this.env.BINDING_BACKEND.fetch(
      "http://binding-backend/internal/assets/v1/fetch",
      { method: "POST", headers, redirect: "manual" },
    );
    if (!response.ok && response.headers.has("x-open-compute-error-code")) {
      throw bindingError(
        response.headers.get("x-open-compute-error-code") ||
          "ASSET_STORAGE_UNAVAILABLE",
      );
    }
    const responseHeaders = new Headers(response.headers);
    const representationLength =
      responseHeaders.get("x-open-compute-asset-representation-length") ??
      responseHeaders.get("content-length");
    if (representationLength) {
      responseHeaders.set(
        "x-open-compute-asset-representation-length",
        representationLength,
      );
    }
    const forwarded = new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers: responseHeaders,
    });
    if (representationLength)
      forwarded.headers.set("content-length", representationLength);
    return forwarded;
  }
}

function doTransportProps(
  props: ResourceBindingProps | undefined,
): ResourceBindingProps {
  if (
    !props ||
    typeof props.accountId !== "string" ||
    typeof props.workerId !== "string" ||
    typeof props.bindingId !== "string" ||
    typeof props.versionId !== "string" ||
    typeof props.namespaceResourceId !== "string" ||
    !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
  ) {
    throw bindingError("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return props;
}

function doTransportHeaders(
  props: ResourceBindingProps,
  objectId: string,
  channelId: string,
  sequence: number,
) {
  if (typeof objectId !== "string" || !/^[0-9a-f]{64}$/.test(objectId)) {
    throw bindingError("DO_ID_INVALID");
  }
  if (
    !DO_ORDER_CHANNEL.test(channelId) ||
    !Number.isSafeInteger(sequence) ||
    sequence < 0
  ) {
    throw bindingError("DO_RUNTIME_EXCEPTION");
  }
  return {
    "x-open-compute-startup-generation": currentStartupGeneration(),
    "x-open-compute-account-id": props.accountId,
    "x-open-compute-worker-id": props.workerId,
    "x-open-compute-binding-id": props.bindingId,
    "x-open-compute-version-id": props.versionId,
    "x-open-compute-descriptor-sha256": props.descriptorSha256,
    "x-open-compute-namespace-resource-id": props.namespaceResourceId,
    "x-open-compute-object-id": objectId,
    "x-open-compute-request-id": crypto.randomUUID(),
    "x-open-compute-do-order-channel": channelId,
    "x-open-compute-do-order-sequence": String(sequence),
  };
}

class DoRpcResult extends RpcTarget {
  #taken = false;
  #value: unknown;

  constructor(value: unknown) {
    super();
    this.#value = value;
  }

  take(): unknown {
    if (this.#taken) throw bindingError("DO_RUNTIME_EXCEPTION");
    this.#taken = true;
    const value = this.#value;
    this.#value = undefined;
    return value;
  }
}

export class DoTransport extends WorkerEntrypoint<
  LoaderEnv,
  ResourceBindingProps
> {
  #props() {
    return doTransportProps(this.ctx.props);
  }

  fetch(request: Request): Promise<Response> {
    const props = this.#props();
    const match = /^\/([0-9a-f]{64})\/([0-9a-f]{32})\/([0-9]+)$/.exec(
      new URL(request.url).pathname,
    );
    if (!match) throw bindingError("DO_RUNTIME_EXCEPTION");
    const objectId = match[1]!;
    const channelId = match[2]!;
    const sequence = Number(match[3]);
    const identity = doTransportHeaders(props, objectId, channelId, sequence);
    const headers = new Headers(request.headers);
    const tenantMethod =
      headers.get("x-open-compute-do-method") || request.method;
    const tenantUrl =
      headers.get("x-open-compute-do-url") || "https://do.invalid/";
    for (const name of INTERNAL_HEADERS) headers.delete(name);
    if (headers.get("upgrade")?.toLowerCase() === "websocket") {
      for (const [name, value] of Object.entries(identity))
        headers.set(name, value);
      headers.set("x-open-compute-do-method", tenantMethod);
      headers.set("x-open-compute-do-url", tenantUrl);
      headers.set("x-open-compute-do-operation", "fetch");
      return this.env.DO_ROUTER.fetch(
        new Request("http://do-router/internal/do/v1/fetch", {
          method: request.method,
          headers,
          body: request.body,
          redirect: "manual",
        }),
      );
    }
    const init: RequestInit = {
      method: request.method,
      headers,
      body: request.body,
      redirect: "manual",
    };
    if (request.method === "GET" || request.method === "HEAD") delete init.body;
    return this.env.DO_ROUTER.dispatchFetch(
      identity,
      new Request(tenantUrl, init),
    );
  }

  startRpc(
    objectId: string,
    channelId: string,
    sequence: number,
    kind: "call" | "get",
    member: string,
    args: unknown[],
  ): DoRpcResult {
    const props = this.#props();
    if (
      (kind !== "call" && kind !== "get") ||
      typeof member !== "string" ||
      !Array.isArray(args) ||
      (kind === "get" && args.length !== 0)
    ) {
      throw bindingError("DO_RPC_UNSUPPORTED");
    }
    const headers = {
      ...doTransportHeaders(props, objectId, channelId, sequence),
      "x-open-compute-do-operation": "rpc",
      "content-type": "application/json",
    };
    const value =
      kind === "call"
        ? this.env.DO_ROUTER.dispatchRpc(headers, member, args)
        : this.env.DO_ROUTER.getRpcProperty(headers, member);
    return new DoRpcResult(value);
  }

  async cancelOrder(
    objectId: string,
    channelId: string,
    sequence: number,
  ): Promise<void> {
    const props = this.#props();
    await this.env.DO_ROUTER.cancelOrder({
      ...doTransportHeaders(props, objectId, channelId, sequence),
      "x-open-compute-do-operation": "rpc",
      "content-type": "application/json",
    });
  }

  async prepareConnect(
    objectId: string,
    channelId: string,
    sequence: number,
    operationId: string,
    authority: SocketAuthorityWire,
  ): Promise<void> {
    const props = this.#props();
    doTransportHeaders(props, objectId, channelId, sequence);
    if (!DO_ORDER_CHANNEL.test(operationId)) {
      throw bindingError("DO_RUNTIME_EXCEPTION");
    }
    const validated = validateSocketAuthorityWire(authority);
    const now = Date.now();
    for (const [token, pending] of doConnects) {
      if (pending.expiresAt <= now) {
        doConnects.delete(token);
        doConnectWaiters.get(token)?.resolve(undefined);
      }
    }
    if (doConnects.size >= 1024) {
      throw bindingError("DO_STORAGE_LIMIT");
    }
    if (doConnects.has(operationId)) throw bindingError("DO_RUNTIME_EXCEPTION");
    const pending = {
      authority: validated,
      channelId,
      expiresAt: now + 10_000,
      objectId,
      sequence,
    };
    doConnects.set(operationId, pending);
    doConnectWaiters.get(operationId)?.resolve(pending);
  }

  async cancelConnect(operationId: string): Promise<void> {
    if (!DO_ORDER_CHANNEL.test(operationId))
      throw bindingError("DO_RUNTIME_EXCEPTION");
    const pending = doConnects.get(operationId);
    if (pending) doConnects.delete(operationId);
    doConnectWaiters.get(operationId)?.resolve(undefined);
  }

  async connect(socket: Socket): Promise<void> {
    try {
      const tokenAddress = await inboundSocketAddress(socket);
      const match = /^([0-9a-f]{32})\.do-transport\.invalid:1$/.exec(
        tokenAddress,
      );
      const pending = match ? await waitForDoConnect(match[1]!) : undefined;
      if (!match || !pending || pending.expiresAt <= Date.now()) {
        if (match && pending) doConnects.delete(match[1]!);
        throw bindingError("DO_RUNTIME_EXCEPTION");
      }
      doConnects.delete(match[1]!);
      const prepared = await this.env.DO_ROUTER.prepareConnect(
        {
          ...doTransportHeaders(
            this.#props(),
            pending.objectId,
            pending.channelId,
            pending.sequence,
          ),
          "x-open-compute-do-operation": "connect",
          "content-type": "application/json",
        },
        pending.authority,
      );
      if (
        !prepared ||
        typeof prepared !== "object" ||
        typeof prepared.tokenAddress !== "string"
      ) {
        throw bindingError("DO_RUNTIME_EXCEPTION");
      }
      const target = this.env.DO_ROUTER.connect(prepared.tokenAddress, {
        allowHalfOpen: true,
      });
      await target.opened;
      await tunnelSockets(socket, target);
    } catch {
      await socket.close().catch(() => undefined);
      throw bindingError("DO_RUNTIME_EXCEPTION");
    }
  }
}

export class AlarmIndex extends WorkerEntrypoint<BindingEnv, AlarmIdentity> {
  #props() {
    const props = this.ctx.props;
    if (
      !props ||
      typeof props.namespaceResourceId !== "string" ||
      typeof props.objectId !== "string" ||
      !/^[0-9a-f]{64}$/.test(props.objectId) ||
      !Number.isSafeInteger(props.objectGeneration) ||
      props.objectGeneration < 1
    ) {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    return props;
  }

  async #request(
    operation: string,
    mutation:
      AlarmProjection | { rowToken: string } | Record<string, never> = {},
  ) {
    const props = this.#props();
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/alarms/v1/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-request-id": crypto.randomUUID(),
        },
        body: JSON.stringify({
          namespaceResourceId: props.namespaceResourceId,
          objectId: props.objectId,
          objectGeneration: props.objectGeneration,
          ...mutation,
        }),
      },
    );
    if (!response.ok) {
      throw bindingError(
        response.headers.get("x-open-compute-error-code") ||
          "DO_ALARM_INDEX_UNAVAILABLE",
      );
    }
  }

  async upsert(row: AlarmProjection) {
    if (
      !row ||
      !Number.isSafeInteger(row.scheduledTimeMs) ||
      row.scheduledTimeMs <= 0 ||
      !Number.isSafeInteger(row.retryCount) ||
      row.retryCount < 0 ||
      row.retryCount > 6 ||
      typeof row.rowToken !== "string"
    ) {
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    }
    await this.#request("upsert", row);
  }

  async delete(rowToken: string) {
    if (typeof rowToken !== "string")
      throw bindingError("SCHEDULER_INTERNAL_PROTOCOL_ERROR");
    await this.#request("delete", { rowToken });
  }

  async clear() {
    await this.#request("clear");
  }
}
