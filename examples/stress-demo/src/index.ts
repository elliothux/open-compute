import { Inventory } from "./do/inventory";
import type { Env } from "./env";
import { InternalApi } from "./internal-api";
import { json, jsonError } from "./lib/json";
import { decodeKeySegment, parseStackRoute } from "./lib/routing";
import { handleMegaCheckout } from "./scenario/mega-checkout";
import { handleScenarioVerify } from "./scenario/verify";
import { handleCpuSpin } from "./stacks/cpu";
import { handleD1Orders } from "./stacks/d1";
import { handleDoIncrement, handleDoWebSocket } from "./stacks/do-stack";
import { handleFetchProbe } from "./stacks/fetch-stack";
import { handleHttpPing } from "./stacks/http";
import { handleKv } from "./stacks/kv";
import { handleQueueDequeueVerify, handleQueueEnqueue } from "./stacks/queue";
import { handleR2Object } from "./stacks/r2";
import { handleServiceCall } from "./stacks/service";
import { handleWorkflowCheckout } from "./stacks/workflow";
import { CheckoutFlow } from "./workflows/checkout";

export { CheckoutFlow, InternalApi, Inventory };

interface QueueMessageBody {
  label: string;
  orderId?: string;
  phase?: string;
  revision?: string;
  index?: number;
  payload?: unknown;
}

async function routeStack(
  request: Request,
  env: Env,
  route: ReturnType<typeof parseStackRoute>,
): Promise<Response> {
  if (!route) {
    return jsonError("not_found", 404);
  }

  switch (route.stack) {
    case "http":
      if (route.resource === "ping") {
        return handleHttpPing(request, env);
      }
      break;
    case "kv": {
      const key = route.resource;
      if (!key) {
        return jsonError("missing_key", 400, { stack: "kv" });
      }
      return handleKv(request, env, decodeKeySegment(key));
    }
    case "d1":
      if (route.resource === "orders") {
        return handleD1Orders(request, env);
      }
      break;
    case "r2": {
      if (route.resource !== "objects") {
        break;
      }
      const key = route.segments[0];
      if (!key) {
        return jsonError("missing_key", 400);
      }
      return handleR2Object(request, env, decodeKeySegment(key));
    }
    case "queue":
      if (route.resource === "enqueue") {
        return handleQueueEnqueue(request, env);
      }
      if (route.resource === "dequeue-verify") {
        return handleQueueDequeueVerify(request, env);
      }
      break;
    case "do": {
      const objectId = route.resource;
      if (!objectId) {
        return jsonError("missing_object_id", 400);
      }
      const decodedId = decodeKeySegment(objectId);
      const action = route.segments[0];
      if (action === "ws") {
        return handleDoWebSocket(request, env, decodedId);
      }
      if (action === "increment" || action === undefined) {
        return handleDoIncrement(request, env, decodedId);
      }
      if (action === "state") {
        return handleDoIncrement(request, env, decodedId);
      }
      break;
    }
    case "workflow":
      if (route.resource === "checkout") {
        return handleWorkflowCheckout(request, env);
      }
      break;
    case "fetch":
      if (route.resource === "probe") {
        return handleFetchProbe(request, env);
      }
      break;
    case "cpu":
      if (route.resource === "spin") {
        return handleCpuSpin(request, env);
      }
      break;
    case "service":
      if (route.resource === "call") {
        return handleServiceCall(request, env);
      }
      break;
    case "scenario":
      if (route.resource === "mega-checkout") {
        return handleMegaCheckout(request, env);
      }
      if (route.resource === "verify") {
        return handleScenarioVerify(request, env);
      }
      break;
    default:
      break;
  }

  return jsonError("not_found", 404, {
    stack: route.stack,
    resource: route.resource,
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url);
    const path = url.pathname;

    if (path === "/api/health") {
      return json({
        ok: true,
        revision: env.REVISION,
        hasSecret: env.TOKEN.length > 0,
        routes: [
          "/stack/http/ping",
          "/stack/kv/:key",
          "/stack/d1/orders",
          "/stack/r2/objects/:key",
          "/stack/queue/enqueue",
          "/stack/queue/dequeue-verify",
          "/stack/do/:id/increment",
          "/stack/do/:id/ws",
          "/stack/workflow/checkout",
          "/stack/fetch/probe",
          "/stack/cpu/spin",
          "/stack/service/call",
          "/stack/scenario/mega-checkout",
          "/stack/scenario/verify",
        ],
      });
    }

    if (path.startsWith("/stack/")) {
      return routeStack(request, env, parseStackRoute(path));
    }

    return jsonError("not_found", 404, { path });
  },

  async queue(batch, env: Env): Promise<void> {
    for (const message of batch.messages) {
      const body = message.body as QueueMessageBody;
      const label = body.label;
      await env.KV.put(`queue-processed:${label}`, body.phase ?? "processed");
      if (body.orderId) {
        await env.KV.put(
          `queue-order:${body.orderId}:${label}`,
          JSON.stringify(body),
        );
      }
      message.ack();
    }
  },
} satisfies ExportedHandler<Env>;
