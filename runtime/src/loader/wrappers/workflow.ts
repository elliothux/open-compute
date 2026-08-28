import { withEnv, WorkerEntrypoint } from "cloudflare:workers";
import type { WorkflowEventWire, WorkflowRunResult } from "../../workflows/execution-protocol.js";
import type { Environment, EnvironmentWrapper } from "./runtime.js";

/** Select the matching runner/controller contract without exposing either in tenant env. */
export function createWorkflowEntrypoint<Controller>(
  target: unknown,
  wrapEnv: EnvironmentWrapper,
  run: (target: unknown, ctx: ExecutionContext, env: Environment, event: WorkflowEventWire, controller: Controller) => Promise<WorkflowRunResult>,
  validate: (target: unknown) => boolean,
) {
  return class extends WorkerEntrypoint<Environment> {
    validate(): boolean { return validate(target); }
    execute(event: WorkflowEventWire, controller: Controller): Promise<WorkflowRunResult> {
      const wrapped = wrapEnv(this.env);
      let pending: Promise<WorkflowRunResult> | undefined;
      withEnv(wrapped, () => { pending = run(target, this.ctx, wrapped, event, controller); });
      if (!pending) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
      return pending;
    }
  };
}
