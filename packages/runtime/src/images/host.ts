import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, ImageTransportProps } from "../bindings/protocol.js";
import {
  bindingJson, expectBindingStatus, framed, isRecord,
} from "../bindings/private-transport.js";
import {
  bindingError, BINDING_TOKEN_HEADER, currentStartupGeneration,
} from "../loader/shared.js";

/** Private version-scoped Images transport; never exposed directly to tenant code. */
export class ImageTransport extends WorkerEntrypoint<BindingEnv, ImageTransportProps> {
  #headers() {
    const props = this.ctx.props;
    if (!props || typeof props.accountId !== "string" || typeof props.workerId !== "string"
        || typeof props.versionId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)) throw bindingError("IMAGE_PROTOCOL_ERROR");
    return {
      [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
      "x-open-compute-startup-generation": currentStartupGeneration(),
      "x-open-compute-account-id": props.accountId,
      "x-open-compute-worker-id": props.workerId,
      "x-open-compute-version-id": props.versionId,
      "x-open-compute-descriptor-sha256": props.descriptorSha256,
      "x-open-compute-request-id": crypto.randomUUID(),
    };
  }

  async #fetch(path: string, init: RequestInit): Promise<Response> {
    const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend${path}`, {
      ...init, headers: { ...this.#headers(), ...(init.headers ?? {}) },
    });
    if (!response.ok) {
      try { await response.body?.cancel(); } catch { /* best effort */ }
      throw bindingError(response.headers.get("x-open-compute-error-code") || "IMAGE_UNAVAILABLE");
    }
    return response;
  }

  async input(body: ReadableStream<Uint8Array>): Promise<string> {
    const response = await this.#fetch("/internal/images/v1/input", { method: "POST", body });
    await expectBindingStatus(response, 200, "IMAGE_PROTOCOL_ERROR");
    const value = await bindingJson(response, "IMAGE_PROTOCOL_ERROR");
    if (!isRecord(value) || Object.keys(value).length !== 1 || typeof value.session !== "string"
        || !/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value.session)) {
      throw bindingError("IMAGE_PROTOCOL_ERROR");
    }
    return value.session;
  }

  async info(body: ReadableStream<Uint8Array>): Promise<unknown> {
    const response = await this.#fetch("/internal/images/v1/info", { method: "POST", body });
    await expectBindingStatus(response, 200, "IMAGE_PROTOCOL_ERROR");
    return bindingJson(response, "IMAGE_PROTOCOL_ERROR");
  }

  async transform(session: string, options: unknown): Promise<void> {
    const response = await this.#fetch(`/internal/images/v1/session/${encodeURIComponent(session)}/transform`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(options),
    });
    await expectBindingStatus(response, 204, "IMAGE_PROTOCOL_ERROR");
    try { await response.body?.cancel(); } catch { /* best effort */ }
  }

  async draw(session: string, body: ReadableStream<Uint8Array>, options: unknown): Promise<void> {
    const response = await this.#fetch(`/internal/images/v1/session/${encodeURIComponent(session)}/draw`, {
      method: "POST", headers: { "content-type": "application/vnd.open-compute.image.v1+frame" },
      body: framed(options, body, "IMAGE_LIMIT_EXCEEDED"),
    });
    await expectBindingStatus(response, 204, "IMAGE_PROTOCOL_ERROR");
    try { await response.body?.cancel(); } catch { /* best effort */ }
  }

  async output(session: string, options: unknown): Promise<Response> {
    const response = await this.#fetch(`/internal/images/v1/session/${encodeURIComponent(session)}/output`, {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(options),
    });
    await expectBindingStatus(response, 200, "IMAGE_PROTOCOL_ERROR");
    return response;
  }
}
