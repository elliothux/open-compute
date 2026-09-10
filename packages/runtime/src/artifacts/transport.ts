import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv, ResourceBindingProps } from "../bindings/protocol.js";
import {
  BINDING_TOKEN_HEADER,
  bindingError,
  currentStartupGeneration,
  systemRequestId,
} from "../loader/shared.js";

const OPERATIONS = new Set([
  "create",
  "get",
  "import",
  "list",
  "delete",
  "create-token",
  "list-tokens",
  "revoke-token",
  "fork",
]);

class ArtifactTransportError extends Error {
  readonly name = "ArtifactsError";
  constructor(
    readonly code: string,
    readonly numericCode: number,
  ) {
    super(code);
  }
}

/** Private immutable Artifacts namespace transport. */
export class ArtifactsTransport extends WorkerEntrypoint<
  BindingEnv,
  ResourceBindingProps
> {
  async call(operation: string, input: unknown): Promise<unknown> {
    if (!OPERATIONS.has(operation))
      throw bindingError("ARTIFACTS_PROTOCOL_ERROR");
    const props = this.ctx.props;
    if (
      !props ||
      typeof props.bindingId !== "string" ||
      typeof props.versionId !== "string" ||
      typeof props.namespaceResourceId !== "string" ||
      !Number.isSafeInteger(props.resourceSpecGeneration) ||
      props.resourceSpecGeneration < 1 ||
      !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
    ) {
      throw bindingError("ARTIFACTS_PROTOCOL_ERROR");
    }
    const response = await this.env.BINDING_BACKEND.fetch(
      `http://binding-backend/internal/bindings/v1/artifacts/${props.bindingId}/${operation}`,
      {
        method: "POST",
        headers: {
          "content-type": "application/json",
          [BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
          "x-open-compute-startup-generation": currentStartupGeneration(),
          "x-open-compute-version-id": props.versionId,
          "x-open-compute-descriptor-sha256": props.descriptorSha256,
          "x-open-compute-request-id": systemRequestId(),
        },
        body: JSON.stringify(input),
      },
    );
    if (!response.ok) {
      const code = response.headers.get("x-open-compute-error-code");
      const numericCode = Number(
        response.headers.get("x-open-compute-error-numeric"),
      );
      await response.body?.cancel().catch(() => undefined);
      throw new ArtifactTransportError(
        code || "INTERNAL_ERROR",
        Number.isSafeInteger(numericCode) ? numericCode : 10400,
      );
    }
    try {
      return await response.json();
    } catch {
      throw bindingError("ARTIFACTS_PROTOCOL_ERROR");
    }
  }
}
