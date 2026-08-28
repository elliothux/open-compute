// Generated from runtime/src/workflows/runner.ts by Rolldown. Do not edit.
import { WorkflowEntrypoint } from "cloudflare:workers";
import { workflowError, workflowFailure, workflowJson, workflowString } from "./json.js";
export function validateWorkflowClass(target) {
	return typeof target === "function" && WorkflowEntrypoint.prototype.isPrototypeOf(target.prototype) && typeof target.prototype.run === "function";
}
// The backend capability stays in this closure, never in tenant env or event.
// Raw run/step tokens remain in the system isolate, outside tenant Promise hooks.
export async function runWorkflow(target, ctx, env, event, backend) {
	if (!validateWorkflowClass(target)) throw workflowError("WORKFLOW_VERSION_NOT_READY");
	let ordinal = 0;
	const counts = new Map();
	let active = false;
	let closed = false;
	let failure;
	let unknown = false;
	const background = [];
	const reject = (code) => {
		failure ||= code;
		throw workflowError(code);
	};
	const rpc = async (operation) => {
		let reply;
		try {
			reply = await operation();
		} catch {
			unknown = true;
			throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
		if (reply?.errorCode) reject(reply.errorCode);
		return reply;
	};
	const unsupported = () => reject("WORKFLOW_METHOD_UNSUPPORTED");
	const step = Object.freeze({
		async do(name, callback, ...extra) {
			if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
			if (closed) return reject("WORKFLOW_RUN_STALE");
			if (active) return reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
			if (typeof callback !== "function" || extra.length) {
				return reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
			}
			try {
				workflowString(name, 256, "WORKFLOW_SERIALIZATION_UNSUPPORTED");
				if (!name) reject("WORKFLOW_SERIALIZATION_UNSUPPORTED");
			} catch (error) {
				failure ||= "WORKFLOW_SERIALIZATION_UNSUPPORTED";
				throw error;
			}
			active = true;
			const index = ordinal++;
			const count = (counts.get(name) || 0) + 1;
			counts.set(name, count);
			try {
				const identity = {
					ordinal: index,
					name,
					nameCount: count,
					configJson: "null"
				};
				const grant = await rpc(() => backend.claim(identity));
				if (grant.state === "complete") return JSON.parse(grant.outputJson);
				if (grant.state === "failed") {
					failure ||= grant.errorCode || "WORKFLOW_EXECUTION_FAILED";
					throw workflowError(grant.error?.message || "Workflow execution failed");
				}
				if (grant.state !== "run") {
					unknown = true;
					throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
				}
				let outputJson;
				let value;
				try {
					value = await callback(Object.freeze({
						step: Object.freeze({
							name,
							count
						}),
						attempt: 1,
						config: null
					}));
				} catch {
					failure ||= "WORKFLOW_EXECUTION_FAILED";
					await rpc(() => backend.failure({
						ordinal: index,
						error: workflowFailure()
					}));
					throw workflowError("Workflow execution failed");
				}
				if (closed) throw workflowError("WORKFLOW_RUN_STALE");
				try {
					outputJson = workflowJson(value);
				} catch (error) {
					const code = error instanceof Error && error.message === "WORKFLOW_RESULT_TOO_LARGE" ? "WORKFLOW_RESULT_TOO_LARGE" : "WORKFLOW_SERIALIZATION_UNSUPPORTED";
					failure ||= code;
					await rpc(() => backend.failure({
						ordinal: index,
						error: workflowFailure(),
						errorCode: code
					}));
					throw workflowError(code);
				}
				await rpc(() => backend.success({
					ordinal: index,
					outputJson
				}));
				return JSON.parse(outputJson);
			} finally {
				active = false;
			}
		},
		sleep: unsupported,
		sleepUntil: unsupported,
		waitForEvent: unsupported
	});
	// The native constructor requires a real ExecutionContext. It contains no run
	// props or backend. Shadow waitUntil before tenant construction so all uses are
	// observed, including ones made in the constructor.
	Object.defineProperty(ctx, "waitUntil", { value(promise) {
		if (closed) reject("WORKFLOW_RUN_STALE");
		const observed = Promise.resolve(promise);
		observed.catch(() => {});
		background.push(observed);
	} });
	let outputJson;
	try {
		const instance = new target(ctx, env);
		const publicEvent = Object.freeze({
			payload: JSON.parse(event.payloadJson),
			timestamp: new Date(event.createdAtMs),
			instanceId: event.externalInstanceId,
			workflowName: event.definitionName
		});
		const value = await instance.run(publicEvent, step);
		if (active) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
		for (let index = 0; index < background.length; index++) await background[index];
		if (active) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
		if (ordinal === 0) reject("WORKFLOW_NON_DETERMINISTIC");
		try {
			outputJson = workflowJson(value);
		} catch (error) {
			reject(error instanceof Error && error.message === "WORKFLOW_RESULT_TOO_LARGE" ? "WORKFLOW_RESULT_TOO_LARGE" : "WORKFLOW_SERIALIZATION_UNSUPPORTED");
		}
	} catch {
		failure ||= "WORKFLOW_EXECUTION_FAILED";
	} finally {
		closed = true;
	}
	if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
	return failure ? {
		outcome: "errored",
		errorCode: failure,
		error: workflowFailure(),
		finalOrdinal: ordinal
	} : {
		outcome: "complete",
		outputJson,
		finalOrdinal: ordinal
	};
}
