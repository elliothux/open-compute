import { WorkflowEntrypoint, type WorkflowEvent, type WorkflowStep } from "cloudflare:workers";

interface Params {
  readonly mode: "surface" | "event" | "batch";
  readonly marker: string;
}

interface Env {
  FLOW: Workflow<Params>;
}

type Status = Awaited<ReturnType<WorkflowInstance["status"]>>;

async function settle(instance: WorkflowInstance): Promise<Status> {
  const deadline = Date.now() + 60_000;
  let current = await instance.status();
  while (current.status !== "complete" && current.status !== "errored" && current.status !== "terminated") {
    if (Date.now() >= deadline) throw new Error("workflow did not settle");
    await scheduler.wait(100);
    current = await instance.status();
  }
  return current;
}

async function remove(binding: Workflow<Params>, ids: readonly string[]): Promise<void> {
  for (const id of ids) {
    try {
      await (await binding.get(id)).delete();
    } catch {
      // Cleanup is idempotent across an observation that already deleted the instance.
    }
  }
}

export class PortableWorkflow extends WorkflowEntrypoint<Env, Params> {
  async run(event: Readonly<WorkflowEvent<Params>>, step: WorkflowStep): Promise<unknown> {
    if (event.payload.mode === "event") {
      const received = await step.waitForEvent<{ ok: boolean }>("approval", {
        type: "approved",
        timeout: "1 minute",
      });
      return { received: received.payload.ok === true && received.type === "approved" };
    }
    const first = await step.do("first", {
      retries: { limit: 1, delay: 0, backoff: "constant" },
      timeout: "1 minute",
    }, async context => ({
      attempt: context.attempt,
      name: context.step.name,
      count: context.step.count,
      marker: event.payload.marker,
    }));
    const structured = await step.do("structured", async () => ({
      when: new Date(0),
      values: new Map([["x", new Set([1, 2])]]),
    }));
    const parallel = await Promise.all([
      step.do("parallel-a", async () => "a"),
      step.do("parallel-b", async () => "b"),
    ]);
    await step.sleep("short-sleep", 1);
    await step.sleepUntil("current-time", new Date());
    return {
      identity: event.instanceId.length > 0 && event.workflowName.length > 0
        && event.timestamp instanceof Date && first.marker === event.payload.marker,
      scheduleAbsent: event.schedule === undefined,
      step: { attempt: first.attempt, name: first.name, count: first.count },
      structured: structured.when instanceof Date
        && structured.values instanceof Map
        && structured.values.get("x") instanceof Set,
      parallel,
      slept: true,
    };
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/surface" && request.method === "POST") {
      const instance = await env.FLOW.create({
        id: "portable-surface",
        params: { mode: "surface", marker: "surface" },
        retention: { successRetention: "1 day", errorRetention: 3600 },
        locationHint: "enam",
      });
      const status = await settle(instance);
      if (status.status !== "complete") throw new Error("surface workflow failed");
      const batch = await env.FLOW.createBatch([
        { id: "portable-batch-a", params: { mode: "batch", marker: "a" }, locationHint: "weur" },
        { id: "portable-batch-b", params: { mode: "batch", marker: "b" } },
      ]);
      const batchStatuses = await Promise.all(batch.map(settle));
      const deleted = await env.FLOW.deleteBatch(batch.map(item => item.id));
      await instance.delete();
      return Response.json({
        instance: { id: instance.id, status: status.status, output: status.output },
        batch: {
          ids: batch.map(item => item.id),
          complete: batchStatuses.every(item => item.status === "complete"),
          deleted: deleted.deleted.length,
          errors: deleted.errors.length,
        },
      });
    }
    if (path === "/event" && request.method === "POST") {
      const instance = await env.FLOW.create({
        id: "portable-event",
        params: { mode: "event", marker: "event" },
      });
      await instance.sendEvent({ type: "approved", payload: { ok: true } });
      const status = await settle(instance);
      if (status.status !== "complete") throw new Error("event workflow failed");
      await instance.delete();
      const output = status.output;
      return Response.json({
        id: instance.id,
        status: status.status,
        received: output !== null && typeof output === "object" && Reflect.get(output, "received") === true,
      });
    }
    if (path === "/cleanup" && request.method === "DELETE") {
      await remove(env.FLOW, ["portable-surface", "portable-batch-a", "portable-batch-b", "portable-event"]);
      return Response.json({ cleaned: true });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
