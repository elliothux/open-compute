import { WorkerEntrypoint } from "cloudflare:workers";
import { bindingError } from "../loader/shared.js";
import { nativeQueueFailure, nativeQueueFetch } from "./native-adapter.js";
import type { QueueBindingProps, QueueTransportEnv } from "./protocol.js";

/** Generation-authenticated Queue authority transport shared by native and durable outbox calls. */
export class QueueTransport extends WorkerEntrypoint<
  QueueTransportEnv,
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
      props.queueLifecycleGeneration < 1 ||
      typeof props.durableObject !== "boolean"
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

  async fetch(request: Request): Promise<Response> {
    try {
      // Native DO sends must use the durable outbox before this route is admitted for that context.
      if (this.#props().durableObject)
        throw bindingError("QUEUE_INVARIANT_VIOLATION");
      return await nativeQueueFetch(request, this, this.env.QUEUE_WIRE_CODEC);
    } catch (cause) {
      return nativeQueueFailure(cause);
    }
  }
}
