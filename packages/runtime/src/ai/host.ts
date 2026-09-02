import { WorkerEntrypoint } from "cloudflare:workers";
import type { AiTransportProps, BindingEnv } from "../bindings/protocol.js";
import { bindingJson, expectBindingStatus, isRecord } from "../bindings/private-transport.js";
import { bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration, systemRequestId } from "../loader/shared.js";

/** Private deployment-scoped Markdown Conversion transport. */
export class AiTransport extends WorkerEntrypoint<BindingEnv, AiTransportProps> {
  #headers(): Record<string, string> {
    const props = this.ctx.props;
    if (!props || typeof props.accountId !== "string" || typeof props.workerId !== "string"
        || typeof props.deploymentId !== "string" || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) {
      throw bindingError("AI_PROTOCOL_ERROR");
    }
    return {
      [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-account-id": props.accountId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-deployment-id": props.deploymentId,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-request-id": systemRequestId(),
      "content-type": "application/json",
    };
  }

  async #fetch(path: string, init: RequestInit): Promise<unknown> {
    const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, {
      ...init, headers: { ...this.#headers(), ...(init.headers ?? {}) },
    });
    if (!response.ok) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(response.headers.get("x-open-compute-error-code") || "AI_UNAVAILABLE");
    }
    await expectBindingStatus(response, 200, "AI_PROTOCOL_ERROR");
    const value = await bindingJson(response, "AI_PROTOCOL_ERROR");
    if (!isRecord(value) || value.schemaVersion !== 1 || !Array.isArray(value.result)
        || Object.keys(value).some(key => !["schemaVersion", "result"].includes(key))) {
      throw bindingError("AI_PROTOCOL_ERROR");
    }
    return value.result;
  }

  transform(files: unknown[], options: unknown): Promise<unknown> {
    return this.#fetch("/internal/ai/to-markdown/v1/transform", {
      method: "POST", body: JSON.stringify({ schemaVersion: 1, files, options }),
    });
  }

  supported(): Promise<unknown> {
    return this.#fetch("/internal/ai/to-markdown/v1/supported", { method: "GET" });
  }
}
