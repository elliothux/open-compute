// Generated from runtime/src/workflows/controller-v2.ts by Rolldown. Do not edit.
import { RpcTarget } from "cloudflare:workers";
import { currentStartupGeneration } from "../loader/host.js";
function record(value) {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
function assertConfig(value) {
	if (!record(value) || typeof value.timeout !== "number" || !Number.isSafeInteger(value.timeout) || !record(value.retries) || typeof value.retries.limit !== "number" || !Number.isSafeInteger(value.retries.limit) || typeof value.retries.delay !== "number" || !Number.isSafeInteger(value.retries.delay) || value.retries.backoff !== "constant" && value.retries.backoff !== "linear" && value.retries.backoff !== "exponential") {
		throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
	}
}
// Symbols are not RPC methods; only the local system host may finalize a run.
export const finishWorkflowRun = Symbol("finishWorkflowRun");
export const closeWorkflowRun = Symbol("closeWorkflowRun");
// A controller belongs to exactly one dispatch RPC. No raw grant, private reply,
// or asynchronous operation returning one ever enters the loaded tenant realm.
export class WorkflowRunControllerV2 extends RpcTarget {
	#env;
	#identity;
	#grants = new Map();
	#yield = false;
	#unknown = false;
	#closed = false;
	#claiming = false;
	#drainIncomplete = false;
	#startedAt;
	#budgetMs;
	constructor(env, identity, budgetMs) {
		super();
		if (!Number.isSafeInteger(budgetMs) || budgetMs < 0 || budgetMs > 36e5) throw new Error("invalid activation budget");
		this.#env = env;
		this.#identity = identity;
		this.#startedAt = performance.now();
		this.#budgetMs = budgetMs;
	}
	[closeWorkflowRun]() {
		this.#closed = true;
		for (const grant of this.#grants.values()) {
			clearTimeout(grant.timer);
			grant.resolve({ errorCode: "WORKFLOW_RUN_STALE" });
			grant.acknowledge();
		}
		this.#grants.clear();
	}
	async #request(operation, body) {
		if (this.#closed || this.#unknown) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
		try {
			const response = await this.#env.BINDING_BACKEND.fetch(`http://binding-backend/internal/workflows/v2/runs/${operation}`, {
				method: "POST",
				headers: {
					"content-type": "application/json",
					"x-open-compute-binding-token": this.#env.BINDING_BACKEND_TOKEN,
					"x-open-compute-startup-generation": currentStartupGeneration()
				},
				body: JSON.stringify({
					...body,
					...this.#identity
				})
			});
			if (!response.ok) {
				const code = response.headers.get("x-open-compute-error-code") || "WORKFLOW_RUN_STALE";
				try {
					await response.body?.cancel();
				} catch {}
				if (response.status >= 500) throw new Error("unknown");
				return { errorCode: code };
			}
			const reply = await response.json();
			if (this.#closed) throw new Error("closed");
			if (!record(reply)) throw new Error("invalid reply");
			return reply;
		} catch {
			this.#unknown = true;
			// Do not expose transport exceptions or retry an ambiguous mutation.
			throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
	}
	#verdict(reply) {
		if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
		switch (reply?.state) {
			case "complete":
				if (reply.outputJson !== undefined && typeof reply.outputJson !== "string") break;
				return {
					state: "complete",
					outputJson: reply.outputJson
				};
			case "failed":
				if (typeof reply.code !== "string") break;
				return {
					state: "failed",
					code: reply.code
				};
			case "suspended":
				this.#yield = true;
				return { state: "suspended" };
		}
		this.#unknown = true;
		throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
	}
	async claimBatch(body) {
		if (this.#drainIncomplete) return { state: "suspended" };
		if (this.#yield) return { state: "suspended" };
		if (this.#claiming || [...this.#grants.values()].some((grant) => !grant.settled || !grant.acknowledged)) {
			return { errorCode: "WORKFLOW_PARALLEL_STEP_UNSUPPORTED" };
		}
		this.#claiming = true;
		try {
			const reply = await this.#request("claim-batch", {
				...body,
				remainingMs: Math.max(0, Math.floor(this.#budgetMs - (performance.now() - this.#startedAt)))
			});
			if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
			if (reply.state === "suspended") {
				this.#yield = true;
				return { state: "suspended" };
			}
			if (!Array.isArray(reply?.steps) || reply.steps.length < 1 || reply.steps.length > 16 || reply.steps.length !== body?.steps?.length) throw new Error("invalid batch");
			this.#grants.clear();
			const steps = [];
			for (let i = 0; i < reply.steps.length; i++) {
				const step = reply.steps[i];
				if (!record(step)) throw new Error("invalid step");
				const ordinal = body.steps[i].ordinal;
				if (step.state !== "run") {
					// Large replay values are fetched individually, not in claim-batch.
					if (step.state !== "complete" && step.state !== "failed" && step.state !== "suspended") throw new Error("invalid state");
					if (step.state === "suspended") this.#yield = true;
					steps.push({
						ordinal,
						state: step.state
					});
					continue;
				}
				if (typeof step.stepToken !== "string" || !/^[0-9a-f]{64}$/.test(step.stepToken) || typeof step.attempt !== "number" || !Number.isInteger(step.attempt) || step.attempt < 1 || step.attempt > 101 || typeof step.remainingMs !== "number" || !Number.isSafeInteger(step.remainingMs) || step.remainingMs < 0 || step.remainingMs > 24e4) {
					throw new Error("invalid grant");
				}
				assertConfig(step.config);
				let resolve;
				const result = new Promise((done) => {
					resolve = done;
				});
				let acknowledge;
				const acknowledgment = new Promise((done) => {
					acknowledge = done;
				});
				const grant = {
					stepToken: step.stepToken,
					attempt: step.attempt,
					result,
					resolve,
					acknowledgment,
					acknowledge,
					acknowledged: false,
					settled: false,
					committing: false,
					timer: null
				};
				this.#grants.set(ordinal, grant);
				grant.timer = setTimeout(() => {
					this.#commit("timeout", { ordinal }).catch(() => {
						grant.resolve({ errorCode: "WORKFLOW_RUNTIME_UNAVAILABLE" });
					});
				}, step.remainingMs);
				steps.push({
					ordinal,
					state: "run",
					attempt: step.attempt,
					config: step.config
				});
			}
			return { steps };
		} catch {
			this.#unknown = true;
			throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
		} finally {
			this.#claiming = false;
		}
	}
	async #commit(operation, body) {
		const grant = this.#grants.get(body?.ordinal);
		if (grant && operation !== "timeout") {
			grant.acknowledged = true;
			grant.acknowledge();
		}
		if (!grant || grant.settled || grant.committing) return { errorCode: "WORKFLOW_STEP_STALE" };
		grant.committing = true;
		clearTimeout(grant.timer);
		try {
			const verdict = this.#verdict(await this.#request(operation, {
				...body,
				stepToken: grant.stepToken,
				attempt: grant.attempt
			}));
			grant.settled = true;
			grant.resolve(verdict);
			return verdict;
		} catch {
			this.#unknown = true;
			grant.resolve({ errorCode: "WORKFLOW_RUNTIME_UNAVAILABLE" });
			throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
	}
	success(body) {
		return this.#commit("success", body);
	}
	failure(body) {
		return this.#commit("failure", body);
	}
	async result(ordinal) {
		const grant = this.#grants.get(ordinal);
		if (grant) return grant.result;
		return this.#verdict(await this.#request("result", { ordinal }));
	}
	async drain() {
		const pending = [...this.#grants.values()].filter((grant) => !grant.acknowledged);
		if (pending.length !== 0) {
			let timer = null;
			// A timeout fences commits, not arbitrary JS or external side effects. Wait
			// a bounded margin for actual callback reports, then quarantine the run.
			const acknowledged = await Promise.race([Promise.all(pending.map((grant) => grant.acknowledgment)).then(() => true), new Promise((resolve) => {
				timer = setTimeout(() => resolve(false), 3e4);
			})]);
			clearTimeout(timer);
			if (!acknowledged) {
				this.#drainIncomplete = true;
				return { state: "suspended" };
			}
		}
		// Do not retain a full batch's serialized results while reading an event or
		// serializing the final output. SQLite remains the replay authority.
		if ([...this.#grants.values()].every((grant) => grant.settled)) this.#grants.clear();
		return { ok: true };
	}
	async registerWait(body) {
		if (this.#drainIncomplete) return { state: "suspended" };
		if (this.#yield) return { state: "suspended" };
		if (this.#claiming || [...this.#grants.values()].some((grant) => !grant.settled || !grant.acknowledged)) {
			return { errorCode: "WORKFLOW_PARALLEL_STEP_UNSUPPORTED" };
		}
		return this.#verdict(await this.#request(body.kind === "wait_event" ? "register-wait" : "register-sleep", body));
	}
	// Called only by the trusted host after the loaded RPC finishes. A forged or
	// caught tenant signal is never authority to yield or to commit success.
	async [finishWorkflowRun](result) {
		if (this.#drainIncomplete) {
			return {
				result: {
					outcome: "unknown",
					finalOrdinal: result.finalOrdinal
				},
				drainIncomplete: true
			};
		}
		if (this.#unknown || this.#closed || this.#claiming || [...this.#grants.values()].some((grant) => !grant.settled)) {
			throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
		}
		if (this.#yield) {
			const reply = await this.#request("yield", { finalOrdinal: result.finalOrdinal });
			if (reply?.ok !== true) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
			return {
				result: {
					outcome: "suspended",
					finalOrdinal: result.finalOrdinal
				},
				drainIncomplete: false
			};
		}
		if (!["complete", "errored"].includes(result?.outcome)) throw new Error("invalid outcome");
		return {
			result,
			drainIncomplete: false
		};
	}
}
