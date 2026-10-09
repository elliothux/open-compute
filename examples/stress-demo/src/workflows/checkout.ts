import {
  WorkflowEntrypoint,
  type WorkflowEvent,
  type WorkflowStep,
} from "cloudflare:workers";
import type { Env } from "../env";

export interface CheckoutParams {
  orderId: string;
  mode: string;
  fanOutN: number;
}

export class CheckoutFlow extends WorkflowEntrypoint<Env, CheckoutParams> {
  async run(
    event: WorkflowEvent<CheckoutParams>,
    step: WorkflowStep,
  ): Promise<Record<string, unknown>> {
    const { orderId, mode, fanOutN } = event.payload;
    const revision = this.env.REVISION;

    const prepared = await step.do("prepare-order", async () => {
      await this.env.KV.put(
        `workflow:${orderId}`,
        JSON.stringify({ phase: "prepared", revision }),
      );
      return { orderId, revision, mode };
    });

    const sideEffects = await step.do("side-effects", async () => {
      const labels: string[] = [];
      for (let index = 0; index < fanOutN; index += 1) {
        const label = `workflow-fanout:${orderId}:${index}`;
        labels.push(label);
        await this.env.KV.put(
          label,
          JSON.stringify({ orderId, index, revision }),
        );
      }
      await this.env.KV.put(
        `workflow:${orderId}`,
        JSON.stringify({ phase: "effects", labels, revision }),
      );
      return { labels, count: labels.length };
    });

    await step.sleep("settle", mode === "peak" ? 0 : 0);

    await step.do("finalize", async () => {
      await this.env.KV.put(
        `workflow:${orderId}`,
        JSON.stringify({ phase: "complete", revision, prepared, sideEffects }),
      );
      return { status: "complete" };
    });

    return { orderId, status: "complete", revision, prepared, sideEffects };
  }
}
