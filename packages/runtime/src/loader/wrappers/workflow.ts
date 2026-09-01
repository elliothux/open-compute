import { WorkerEntrypoint } from "cloudflare:workers";
import type { WorkflowEventWire, WorkflowRunResult } from "../../workflows/execution-protocol.js";
import {
  invokeEntrypoint, trackExecutionContext, trustedContextExports,
  type Environment, type EnvironmentWrapper, type TrackedContext,
} from "./runtime.js";

/** Select the matching runner/controller contract without exposing either in tenant env. */
export function createWorkflowEntrypoint<Controller>(
  target: unknown,
  wrapEnv: EnvironmentWrapper,
  run: (target: unknown, ctx: ExecutionContext, env: Environment, event: WorkflowEventWire, controller: Controller) => Promise<WorkflowRunResult>,
  validate: (target: unknown) => boolean,
) {
  return class extends WorkerEntrypoint<Environment> {
    #tracked: TrackedContext<ExecutionContext> | undefined;

    validate(): boolean { return validate(target); }
    execute(event: WorkflowEventWire, controller: Controller): Promise<WorkflowRunResult> {
      const trustedExports = trustedContextExports(this.ctx);
      const wrapped = wrapEnv(this.env);
      const tracked = this.#tracked ??= trackExecutionContext(
        this.ctx, undefined, undefined, false, trustedExports,
      );
      const pending = invokeEntrypoint(this, () =>
        run(target, this.ctx, wrapped, event, controller), [],
      wrapped, tracked);
      if (!(pending instanceof Promise)) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
      return pending as Promise<WorkflowRunResult>;
    }
  };
}
