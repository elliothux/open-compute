// Generated from runtime/src/loader/wrappers/workflow.ts by Rolldown. Do not edit.
import { withEnv, WorkerEntrypoint } from "cloudflare:workers";
/** Select the matching runner/controller contract without exposing either in tenant env. */
export function createWorkflowEntrypoint(target, wrapEnv, run, validate) {
	return class extends WorkerEntrypoint {
		validate() {
			return validate(target);
		}
		execute(event, controller) {
			const wrapped = wrapEnv(this.env);
			let pending;
			withEnv(wrapped, () => {
				pending = run(target, this.ctx, wrapped, event, controller);
			});
			if (!pending) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
			return pending;
		}
	};
}
