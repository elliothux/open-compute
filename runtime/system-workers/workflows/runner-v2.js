// Generated from runtime/src/workflows/runner-v2.ts by Rolldown. Do not edit.
import { WorkflowEntrypoint } from "cloudflare:workers";
import { NonRetryableError } from "cloudflare:workflows";
import { workflowError, workflowJson, workflowString, workflowSerializationCode } from "./json-v2.js";
function isCallback(value) {
	return typeof value === "function";
}
export function validateWorkflowClass(target) {
	return typeof target === "function" && WorkflowEntrypoint.prototype.isPrototypeOf(target.prototype) && typeof target.prototype.run === "function";
}
// The system controller owns timeout and all grants. The local marker only
// unwinds cooperative run() code; catching it cannot reopen the controller.
export async function runWorkflow(target, ctx, env, event, backend) {
	if (!validateWorkflowClass(target)) throw workflowError("WORKFLOW_VERSION_NOT_READY");
	const suspension = Object.freeze(Object.create(null));
	let ordinal = 0;
	let closed = false;
	let suspended = false;
	let unknown = false;
	let failure;
	let collecting = null;
	let active = false;
	let frontier = [];
	const counts = new Map();
	const pending = new Set();
	const background = [];
	const settledFailures = new WeakMap();
	const rememberFailure = settledFailures.set.bind(settledFailures);
	const recalledFailure = settledFailures.get.bind(settledFailures);
	const reject = (code) => {
		failure ||= code;
		throw workflowError(code);
	};
	const check = () => {
		if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		if (suspended) throw suspension;
		if (closed) reject("WORKFLOW_RUN_STALE");
		if (active) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
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
	const report = async (operation) => {
		// A callback reporting after the trusted timeout may only acknowledge its
		// drain. The independent result RPC carries the durable verdict.
		let reply;
		try {
			reply = await operation();
		} catch {
			unknown = true;
			throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
		if (reply?.errorCode && reply.errorCode !== "WORKFLOW_STEP_STALE") reject(reply.errorCode);
	};
	const value = (reply) => {
		if (reply.state === "suspended") {
			suspended = true;
			throw suspension;
		}
		if (reply.state === "failed") {
			if (![
				"WORKFLOW_STEP_TIMEOUT",
				"WORKFLOW_STEP_RETRIES_EXHAUSTED",
				"WORKFLOW_NON_RETRYABLE",
				"WORKFLOW_EVENT_TIMEOUT"
			].includes(reply.code)) {
				reject(reply.code);
			}
			const error = reply.code === "WORKFLOW_NON_RETRYABLE" ? new NonRetryableError("Workflow step is not retryable") : workflowError(reply.code);
			error.stack = `${error.name}: ${error.message}`;
			rememberFailure(error, reply.code);
			throw error;
		}
		if (reply.state !== "complete") {
			unknown = true;
			throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
		try {
			return reply.outputJson === undefined ? undefined : JSON.parse(reply.outputJson);
		} catch {
			unknown = true;
			throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
	};
	const descriptor = (kind, name, config) => {
		check();
		try {
			workflowString(name, 256, "WORKFLOW_SERIALIZATION_UNSUPPORTED");
		} catch {
			reject("WORKFLOW_SERIALIZATION_UNSUPPORTED");
		}
		if (!name) reject("WORKFLOW_SERIALIZATION_UNSUPPORTED");
		const key = `${kind}/${name}`;
		const nameCount = (counts.get(key) || 0) + 1;
		counts.set(key, nameCount);
		return {
			ordinal: ordinal++,
			kind,
			name,
			nameCount,
			config,
			dependencies: [...frontier]
		};
	};
	const execute = async (batch) => {
		collecting = null;
		active = true;
		try {
			const reply = await rpc(() => backend.claimBatch({ steps: batch.map((item) => ({
				...item.descriptor,
				batchFirstOrdinal: batch[0].descriptor.ordinal,
				batchSize: batch.length
			})) }));
			if (reply.state === "suspended") {
				suspended = true;
				throw suspension;
			}
			if (!Array.isArray(reply.steps) || reply.steps.length !== batch.length) {
				unknown = true;
				throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
			}
			const outcomes = await Promise.all(batch.map(async (item, index) => {
				const grant = reply.steps[index];
				const indexOrdinal = item.descriptor.ordinal;
				if (grant.state === "run") {
					// This task is observed but is not the timeout authority. Its late
					// report is rejected by the controller and the durable step fence.
					const callback = (async () => {
						let result;
						try {
							result = await item.callback(Object.freeze({
								step: Object.freeze({
									name: item.descriptor.name,
									count: item.descriptor.nameCount
								}),
								attempt: grant.attempt,
								config: Object.freeze({
									...grant.config,
									retries: Object.freeze(grant.config.retries)
								})
							}));
						} catch (error) {
							// Do not read error.message/name/stack/cause or invoke getters.
							let code = failure || "WORKFLOW_EXECUTION_FAILED";
							try {
								if (!failure && error instanceof NonRetryableError) code = "WORKFLOW_NON_RETRYABLE";
							} catch {}
							await report(() => backend.failure({
								ordinal: indexOrdinal,
								code
							}));
							return;
						}
						let outputJson;
						try {
							outputJson = workflowJson(result);
						} catch (error) {
							const code = workflowSerializationCode(error);
							failure ||= code;
							await report(() => backend.failure({
								ordinal: indexOrdinal,
								code: failure || code
							}));
							return;
						}
						await report(() => backend.success({
							ordinal: indexOrdinal,
							outputJson
						}));
					})();
					callback.catch(() => {});
				}
				try {
					return {
						ok: true,
						value: value(await rpc(() => backend.result(indexOrdinal)))
					};
				} catch (error) {
					return {
						ok: false,
						error
					};
				}
			}));
			const drain = await rpc(() => backend.drain());
			if (drain.state === "suspended") suspended = true;
			frontier = batch.map((item) => item.descriptor.ordinal);
			active = false;
			for (let i = 0; i < batch.length; i++) {
				const outcome = outcomes[i];
				if (suspended) batch[i].reject(suspension);
				else if (outcome.ok) batch[i].resolve(outcome.value);
				else batch[i].reject(outcome.error);
			}
		} catch (error) {
			active = false;
			for (const item of batch) item.reject(error);
		}
	};
	const wait = async (kind, name, config) => {
		const item = descriptor(kind, name, config);
		if (collecting) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
		active = true;
		try {
			const reply = await rpc(() => backend.registerWait({
				...item,
				batchFirstOrdinal: item.ordinal,
				batchSize: 1
			}));
			frontier = [item.ordinal];
			const result = value(reply);
			if (kind !== "wait_event") return undefined;
			if (result === null || typeof result !== "object" || !("type" in result) || typeof result.type !== "string" || !("payload" in result) || !("timestampMs" in result) || typeof result.timestampMs !== "number" || !Number.isSafeInteger(result.timestampMs)) {
				unknown = true;
				throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
			}
			return {
				type: result.type,
				payload: result.payload,
				timestamp: new Date(result.timestampMs)
			};
		} finally {
			active = false;
		}
	};
	const step = Object.freeze({
		do(name, config, callback, ...extra) {
			if (isCallback(config) && callback === undefined) {
				callback = config;
				config = {};
			}
			if (typeof callback !== "function" || extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
			const item = descriptor("do", name, config);
			let resolve;
			let rejectResult;
			const result = new Promise((yes, no) => {
				resolve = yes;
				rejectResult = no;
			});
			// Observe every API promise, including ones the tenant forgets to await.
			result.catch(() => {});
			if (!collecting) {
				collecting = [];
				const batch = collecting;
				queueMicrotask(() => {
					const task = execute(batch);
					pending.add(task);
					task.finally(() => pending.delete(task));
				});
			}
			collecting.push({
				descriptor: item,
				callback,
				resolve,
				reject: rejectResult
			});
			return result;
		},
		sleep(name, duration, ...extra) {
			if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
			return wait("sleep", name, { duration });
		},
		sleepUntil(name, timestamp, ...extra) {
			if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
			return wait("sleep_until", name, { timestamp: timestamp instanceof Date ? timestamp.getTime() : timestamp });
		},
		waitForEvent(name, options, ...extra) {
			if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
			return wait("wait_event", name, options);
		}
	});
	Object.defineProperty(ctx, "waitUntil", { value(promise) {
		if (closed || suspended) reject("WORKFLOW_RUN_STALE");
		const observed = Promise.resolve(promise);
		observed.catch(() => {});
		background.push(observed);
	} });
	let outputJson;
	try {
		const instance = new target(ctx, env);
		const result = await instance.run(Object.freeze({
			payload: JSON.parse(event.payloadJson),
			timestamp: new Date(event.createdAtMs),
			instanceId: event.externalInstanceId,
			workflowName: event.definitionName
		}), step);
		if (collecting || active) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
		for (let index = 0; index < background.length; index++) await background[index];
		if (collecting || active) reject("WORKFLOW_PARALLEL_STEP_UNSUPPORTED");
		try {
			outputJson = workflowJson(result);
		} catch (error) {
			failure ||= workflowSerializationCode(error);
		}
	} catch (error) {
		if (error !== suspension) {
			const code = error !== null && (typeof error === "object" || typeof error === "function") ? recalledFailure(error) : undefined;
			failure ||= code || "WORKFLOW_EXECUTION_FAILED";
		}
	}
	// Sibling commits must finish before terminal/yield; tenant Promise.all's
	// first rejection cannot discard the other callbacks' durable results.
	await Promise.all([...pending]);
	closed = true;
	if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
	return suspended ? {
		outcome: "suspended",
		finalOrdinal: ordinal
	} : failure ? {
		outcome: "errored",
		errorCode: failure,
		finalOrdinal: ordinal
	} : {
		outcome: "complete",
		outputJson,
		finalOrdinal: ordinal
	};
}
