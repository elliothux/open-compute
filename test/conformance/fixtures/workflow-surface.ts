import { WorkflowEntrypoint, type WorkflowEvent, type WorkflowStep } from "cloudflare:workers";

interface Params { readonly key: string }
interface Env { FLOW: Workflow<Params> }

export class PortableWorkflow extends WorkflowEntrypoint<Env, Params> {
  async run(event: Readonly<WorkflowEvent<Params>>, step: WorkflowStep): Promise<unknown> {
    const first = await step.do("first", async context => ({
      key: event.payload.key,
      attempt: context.attempt,
      name: context.step.name,
      count: context.step.count,
    }));
    const dynamic = await step.do("dynamic", {
      retries: {
        limit: 3,
        backoff: "exponential",
        delay: async ({ ctx, error }) => error.name && ctx.attempt > 1 ? "2 seconds" : 1000,
      },
      timeout: "1 minute",
      sensitive: "output",
    }, async context => ({ attempt: context.attempt }), {
      rollback: async ({ ctx, error, output, stepName }) => {
        void ctx;
        void error;
        void output;
        void stepName;
      },
      rollbackConfig: { retries: { limit: 1, delay: 0 }, timeout: 1000 },
    });
    const staticDelay = step.do("static", {
      retries: { limit: 2, delay: "1 second", backoff: "linear" },
    }, async context => context.config.retries?.delay);
    const sleeping = step.sleep("sleep", 1);
    const until = step.sleepUntil("until", new Date());
    const waiting = step.waitForEvent<{ value: number }>("event", { type: "ready", timeout: "1 hour" });
    const [, , eventResult] = await Promise.all([sleeping, until, waiting]);
    return { first, dynamic, staticDelay: await staticDelay, eventResult };
  }
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const created: WorkflowInstance = await env.FLOW.create({
      id: "one",
      params: { key: "value" },
      retention: { successRetention: "1 day", errorRetention: 3600 },
      locationHint: "enam",
    });
    const batch: WorkflowInstance[] = await env.FLOW.createBatch([
      { id: "two", params: { key: "a" }, locationHint: "weur" },
      { id: "three", params: { key: "b" } },
    ]);
    const fetched: WorkflowInstance = await env.FLOW.get(created.id);
    await fetched.pause();
    await fetched.resume();
    await fetched.sendEvent({ type: "ready", payload: new Map([["ok", true]]) });
    await fetched.restart({ from: { name: "first", count: 1, type: "do" } });
    await fetched.terminate({ rollback: true });
    const status: InstanceStatus = await fetched.status();
    await fetched.delete();
    const deleted: WorkflowBatchDeleteResult = await env.FLOW.deleteBatch(batch.map(item => item.id));
    return Response.json({ status: status.status, deleted: deleted.deleted.length, errors: deleted.errors.length });
  },
} satisfies ExportedHandler<Env>;
