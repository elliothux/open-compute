import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, ResourceBindingProps } from "../bindings/protocol.js";
import { bindingJson, expectBindingStatus, isRecord } from "../bindings/private-transport.js";
import {
  bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration, systemRequestId,
} from "../loader/shared.js";

/** Private immutable Vectorize resource transport. */
export class VectorizeTransport extends WorkerEntrypoint<BindingEnv, ResourceBindingProps> {
  #headers(contentType: string): Record<string, string> {
    const props = this.ctx.props;
    if (!props || typeof props.bindingId !== "string" || typeof props.versionId !== "string"
        || typeof props.namespaceResourceId !== "string" || !Number.isSafeInteger(props.resourceSpecGeneration)
        || props.resourceSpecGeneration < 1 || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) {
      throw bindingError("VECTORIZE_PROTOCOL_ERROR");
    }
    return {
      [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-binding-id": props.bindingId,
      "x-open-compute-version-id": props.versionId,
      "x-open-compute-resource-id": props.namespaceResourceId,
      "x-open-compute-resource-generation": String(props.resourceSpecGeneration),
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-request-id": systemRequestId(),
      "content-type": contentType,
    };
  }
  async #response(path: string, body: BodyInit, contentType: string): Promise<unknown> {
    const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, {
      method: "POST", headers: this.#headers(contentType), body,
    });
    if (!response.ok) throw bindingError(response.headers.get("x-open-compute-error-code") || "VECTORIZE_UNAVAILABLE");
    await expectBindingStatus(response, 200, "VECTORIZE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "VECTORIZE_PROTOCOL_ERROR");
    if (!isRecord(value) || value.schemaVersion !== 1 || !Object.hasOwn(value, "result")
        || Object.keys(value).some(key => !["schemaVersion", "result"].includes(key))) throw bindingError("VECTORIZE_PROTOCOL_ERROR");
    return value.result;
  }
  call(operation: string, payload: unknown): Promise<unknown> {
    if (!["describe", "query", "queryById", "deleteByIds", "getByIds"].includes(operation)) throw bindingError("VECTORIZE_PROTOCOL_ERROR");
    return this.#response("/internal/vectorize/v1/call", JSON.stringify({ operation, payload }), "application/json");
  }
  mutate(operation: string, frame: ReadableStream<Uint8Array>): Promise<unknown> {
    if (!['insert', 'upsert'].includes(operation) || !(frame instanceof ReadableStream)) throw bindingError("VECTORIZE_PROTOCOL_ERROR");
    return this.#response(`/internal/vectorize/v1/mutate/${operation}`, frame, "application/vnd.open-compute.vectorize.v1+frame");
  }
}
