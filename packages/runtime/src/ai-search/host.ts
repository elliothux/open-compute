import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, ResourceBindingProps } from "../bindings/protocol.js";
import { bindingJson, expectBindingStatus, framed, isRecord } from "../bindings/private-transport.js";
import {
  bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration, systemRequestId,
} from "../loader/shared.js";

/** Private immutable AI Search resource transport. */
export class AiSearchTransport extends WorkerEntrypoint<BindingEnv, ResourceBindingProps & { instance?: string }> {
  #headers(contentType: string): Record<string, string> {
    const props = this.ctx.props;
    if (!props || typeof props.bindingId !== "string" || typeof props.versionId !== "string"
        || typeof props.namespaceResourceId !== "string" || !Number.isSafeInteger(props.resourceSpecGeneration)
        || props.resourceSpecGeneration < 1 || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) throw bindingError("AI_SEARCH_PROTOCOL_ERROR");
    return {
      [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-binding-id": props.bindingId,
      "x-open-compute-version-id": props.versionId,
      "x-open-compute-resource-id": props.namespaceResourceId,
      "x-open-compute-resource-generation": String(props.resourceSpecGeneration),
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-request-id": systemRequestId(), "content-type": contentType,
    };
  }
  get instance(): string | undefined { return this.ctx.props?.instance; }
  async #fetch(path: string, init: RequestInit, contentType = "application/json"): Promise<Response> {
    const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, { ...init, headers: this.#headers(contentType) });
    if (!response.ok) throw bindingError(response.headers.get("x-open-compute-error-code") || "AI_SEARCH_UNAVAILABLE");
    return response;
  }
  async call(operation: string, instance: string | undefined, payload: unknown): Promise<unknown> {
    if (!["namespace.list", "namespace.create", "namespace.delete", "namespace.search", "namespace.chatCompletions",
      "instance.search", "instance.chatCompletions", "instance.update", "instance.info", "instance.stats",
      "items.list", "items.delete", "item.info", "item.sync", "item.logs", "item.chunks",
      "jobs.list", "jobs.create", "job.info", "job.logs", "job.cancel"].includes(operation)) {
      throw bindingError("AI_SEARCH_PROTOCOL_ERROR");
    }
    const response = await this.#fetch("/internal/ai-search/v1/call", { method: "POST", body: JSON.stringify({ operation, instance, payload }) });
    await expectBindingStatus(response, 200, "AI_SEARCH_PROTOCOL_ERROR"); const value = await bindingJson(response, "AI_SEARCH_PROTOCOL_ERROR");
    if (!isRecord(value) || value.schemaVersion !== 1 || !Object.hasOwn(value, "result") || Object.keys(value).some(key => !["schemaVersion", "result"].includes(key))) throw bindingError("AI_SEARCH_PROTOCOL_ERROR");
    return value.result;
  }
  async stream(operation: string, instance: string | undefined, payload: unknown): Promise<Response> {
    if (!["namespace.chatCompletions", "instance.chatCompletions"].includes(operation)) throw bindingError("AI_SEARCH_PROTOCOL_ERROR");
    const response = await this.#fetch("/internal/ai-search/v1/stream", { method: "POST", body: JSON.stringify({ operation, instance, payload }) });
    await expectBindingStatus(response, 200, "AI_SEARCH_PROTOCOL_ERROR"); if (response.body === null) throw bindingError("AI_SEARCH_PROTOCOL_ERROR"); return response;
  }
  async upload(instance: string | undefined, name: string, contentType: string, body: ReadableStream<Uint8Array>, options: unknown): Promise<unknown> {
    const response = await this.#fetch("/internal/ai-search/v1/upload", { method: "POST", body: framed({ schemaVersion: 1, instance, name, contentType, options }, body, "AI_SEARCH_LIMIT_EXCEEDED") }, "application/vnd.open-compute.ai-search.v1+frame");
    await expectBindingStatus(response, 200, "AI_SEARCH_PROTOCOL_ERROR"); const value = await bindingJson(response, "AI_SEARCH_PROTOCOL_ERROR");
    if (!isRecord(value) || value.schemaVersion !== 1 || !Object.hasOwn(value, "result")) throw bindingError("AI_SEARCH_PROTOCOL_ERROR"); return value.result;
  }
  download(instance: string | undefined, itemId: string): Promise<Response> {
    return this.#fetch("/internal/ai-search/v1/download", { method: "POST", body: JSON.stringify({ instance, itemId }) });
  }
}
