import { WorkerEntrypoint } from "cloudflare:workers";
import type { BindingEnv } from "../bindings/protocol.js";
import {
  BINDING_TOKEN_HEADER,
  bindingError,
  currentStartupGeneration,
} from "../loader/shared.js";

/** Immutable Browser binding identity; internal backend credentials remain in BindingEnv. */
export interface BrowserTransportProps {
  instanceId: string;
  workerId: string;
  versionId: string;
  deploymentId?: string | undefined;
  bindingName: string;
  descriptorSha256: string;
  capabilityVersion: 1;
}

/** Private scoped HTTP/WebSocket transport, always overwriting caller-supplied identity. */
export class BrowserTransport extends WorkerEntrypoint<
  BindingEnv,
  BrowserTransportProps
> {
  fetch(request: Request): Promise<Response> {
    const props = this.ctx.props;
    if (
      !props ||
      !/^[0-9a-f]{64}$/.test(props.descriptorSha256) ||
      props.capabilityVersion !== 1
    )
      throw bindingError("BROWSER_UNAVAILABLE");
    if (request.headers.has("cf-brapi-guardrails"))
      throw bindingError("BROWSER_UNSUPPORTED");
    const url = new URL(request.url);
    if (!url.pathname.startsWith("/v1/"))
      throw bindingError("BROWSER_UNSUPPORTED");
    const headers = new Headers();
    for (const name of ["content-type", "upgrade", "sec-websocket-protocol"])
      if (request.headers.has(name))
        headers.set(name, request.headers.get(name)!);
    headers.set(BINDING_TOKEN_HEADER, this.env.BINDING_BACKEND_TOKEN);
    headers.set(
      "x-open-compute-startup-generation",
      currentStartupGeneration(),
    );
    headers.set("x-open-compute-instance-id", props.instanceId);
    headers.set("x-open-compute-worker-id", props.workerId);
    headers.set("x-open-compute-version-id", props.versionId);
    headers.set("x-open-compute-binding-name", props.bindingName);
    headers.set("x-open-compute-descriptor-sha256", props.descriptorSha256);
    headers.set("x-open-compute-capability-version", "1");
    if (props.deploymentId)
      headers.set("x-open-compute-deployment-id", props.deploymentId);
    return this.env.BINDING_BACKEND.fetch(
      new Request(
        `http://binding-backend/internal/browser${url.pathname}${url.search}`,
        {
          method: request.method,
          headers,
          body: request.body,
          redirect: "manual",
        },
      ),
    );
  }
}
