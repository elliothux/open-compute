import { handleWorkflow } from "../workflows/host.js";
import {
  handleDispatch,
  handleQueue,
  handleScheduled,
  validateDurableObjectClass,
} from "./dispatch.js";
import { revokeWorkerLoaders } from "./namespaces.js";
import type { LoaderEnv } from "./protocol.js";

export { KVNamespace } from "../kv/transport.js";

export { modulesFor } from "./modules.js";
export {
  AlarmIndex,
  ArtifactsTransport,
  AssetTransport,
  D1Transport,
  DoTransport,
  ObservabilityTail,
  QueueTransport,
  R2Transport,
} from "./transports.js";

export { tenantEnv } from "./bindings.js";
export { WorkflowBindingTransport } from "../workflows/binding.js";

export {
  bindingError,
  currentStartupGeneration,
  doPolicy,
  lockWorkerCode,
  resolveSnapshot,
  snapshotWorkerCode,
  tenantGlobalOutbound,
} from "./shared.js";
export {
  ServiceTransport,
  ServiceFetchCompletion,
} from "../services/transport.js";
export { CacheTransport } from "../cache/host.js";
export { ImageTransport } from "../images/host.js";
export { AiTransport } from "../ai/host.js";
export { VectorizeTransport } from "../vectorize/host.js";
export { AiSearchTransport } from "../ai-search/host.js";
export default {
  async fetch(
    request: Request,
    env: LoaderEnv,
    ctx: ExecutionContext,
  ): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (
      request.method === "POST" &&
      path === "/internal/worker-loaders/revoke"
    ) {
      return revokeWorkerLoaders(request, env.WORKER_LOADER_FACTORY);
    }
    if (
      request.method === "POST" &&
      ["/internal/workflow", "/internal/validate-workflow"].includes(path)
    ) {
      return handleWorkflow(
        request,
        env,
        ctx,
        path === "/internal/validate-workflow",
      );
    }
    if (
      path === "/internal/dispatch" &&
      (request.method === "POST" ||
        (request.method === "GET" &&
          request.headers.get("upgrade")?.toLowerCase() === "websocket"))
    ) {
      return handleDispatch(request, env, ctx, false);
    }
    if (request.method === "POST" && path === "/internal/queue") {
      return handleQueue(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/scheduled") {
      return handleScheduled(request, env, ctx);
    }
    if (request.method === "POST" && path === "/internal/validate")
      return handleDispatch(request, env, ctx, true);
    if (request.method === "POST" && path === "/internal/validate-do") {
      return validateDurableObjectClass(request, env);
    }
    return new Response(null, { status: 404 });
  },
};
