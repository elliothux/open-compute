import type { Env } from "../env";
import { jsonError, ok, readJson } from "../lib/json";

interface EnqueueBody {
  label?: string;
  batch?: Array<{ label: string; payload?: unknown }>;
  payload?: unknown;
  delaySeconds?: number;
}

export async function handleQueueEnqueue(
  request: Request,
  env: Env,
): Promise<Response> {
  if (request.method !== "POST") {
    return jsonError("method_not_allowed", 405);
  }

  const body = await readJson<EnqueueBody>(request);
  const delaySeconds = body.delaySeconds ?? 0;

  if (body.batch && body.batch.length > 0) {
    await env.EVENTS.sendBatch(
      body.batch.map((entry) => ({
        body: {
          label: entry.label,
          payload: entry.payload ?? null,
          revision: env.REVISION,
        },
      })),
      { delaySeconds },
    );
    return ok({ stack: "queue", enqueued: body.batch.length, batch: true });
  }

  const label = body.label ?? `queue-${crypto.randomUUID()}`;
  await env.EVENTS.send(
    {
      label,
      payload: body.payload ?? null,
      revision: env.REVISION,
    },
    { delaySeconds },
  );
  return ok({ stack: "queue", enqueued: 1, label, batch: false });
}

export async function handleQueueDequeueVerify(
  request: Request,
  env: Env,
): Promise<Response> {
  const url = new URL(request.url);
  const label = url.searchParams.get("label");
  if (!label) {
    return jsonError("missing_label", 400);
  }

  const processed = await env.KV.get(`queue-processed:${label}`);

  return ok({
    stack: "queue",
    label,
    processed: processed !== null,
    processedPhase: processed,
  });
}
