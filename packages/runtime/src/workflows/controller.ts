import { RpcTarget } from "cloudflare:workers";
import { currentStartupGeneration } from "../loader/host.js";
import type { BindingEnv } from "../bindings/protocol.js";
import type { WorkflowBatchReply, WorkflowBatchStep, WorkflowClaimDeclaration, WorkflowController, WorkflowDrainReply, WorkflowResolvedConfig, WorkflowRunIdentity, WorkflowRunResult, WorkflowVerdict } from "./execution-protocol.js";

interface Grant {
  stepToken: string;
  attempt: number;
  result: Promise<WorkflowVerdict>;
  resolve(value: WorkflowVerdict): void;
  acknowledgment: Promise<void>;
  acknowledge(): void;
  acknowledged: boolean;
  settled: boolean;
  committing: boolean;
  timer: number | null;
}

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function assertConfig(value: unknown): asserts value is WorkflowResolvedConfig {
  if (!record(value) || typeof value.timeout !== "number" || !Number.isSafeInteger(value.timeout)
      || !record(value.retries) || typeof value.retries.limit !== "number" || !Number.isSafeInteger(value.retries.limit)
      || (value.retries.delay !== undefined
        && (typeof value.retries.delay !== "number" || !Number.isSafeInteger(value.retries.delay)))
      || (value.sensitive !== undefined && value.sensitive !== "output")
      || (value.retries.backoff !== "constant" && value.retries.backoff !== "linear" && value.retries.backoff !== "exponential")) {
    throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
  }
}

// Symbols are not RPC methods; only the local system host may finalize a run.
export const finishWorkflowRun = Symbol("finishWorkflowRun");
export const closeWorkflowRun = Symbol("closeWorkflowRun");

// A controller belongs to exactly one dispatch RPC. No raw grant, private reply,
// or asynchronous operation returning one ever enters the loaded tenant realm.
export class WorkflowRunController extends RpcTarget implements WorkflowController {
  #env: BindingEnv;
  #identity: WorkflowRunIdentity;
  #grants = new Map<number, Grant>();
  #yield = false;
  #unknown = false;
  #closed = false;
  #claiming = false;
  #drainIncomplete = false;
  #startedAt;
  #budgetMs;

  constructor(env: BindingEnv, identity: WorkflowRunIdentity, budgetMs: number) {
    super();
    if (!Number.isSafeInteger(budgetMs) || budgetMs < 0 || budgetMs > 3600000) throw new Error("invalid activation budget");
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

  async #request(operation: string, body: object): Promise<Record<string, unknown>> {
    if (this.#closed || this.#unknown) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    try {
      const response = await this.#env.BINDING_BACKEND.fetch(
        `http://binding-backend/internal/workflows/runs/${operation}`, {
          method: "POST",
          headers: {
            "content-type": "application/json",
            "x-open-compute-binding-token": this.#env.BINDING_BACKEND_TOKEN,
            "x-open-compute-startup-generation": currentStartupGeneration(),
          },
          body: JSON.stringify({ ...body, ...this.#identity }),
        },
      );
      if (!response.ok) {
        const code = response.headers.get("x-open-compute-error-code") || "WORKFLOW_RUN_STALE";
        try { await response.body?.cancel(); } catch { /* response already closed */ }
        if (response.status >= 500) throw new Error("unknown");
        return { errorCode: code };
      }
      const reply: unknown = await response.json();
      if (this.#closed) throw new Error("closed");
      if (!record(reply)) throw new Error("invalid reply");
      return reply;
    } catch {
      this.#unknown = true;
      // Do not expose transport exceptions or retry an ambiguous mutation.
      throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
  }

  #verdict(reply: Record<string, unknown>): WorkflowVerdict {
    if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
    switch (reply?.state) {
      case "complete":
        if (reply.outputBase64 !== undefined && typeof reply.outputBase64 !== "string") break;
        return { state: "complete", outputBase64: reply.outputBase64 };
      case "event":
        if (typeof reply.type !== "string" || typeof reply.payloadBase64 !== "string"
            || typeof reply.timestampMs !== "number" || !Number.isSafeInteger(reply.timestampMs)) break;
        return { state: "event", type: reply.type, payloadBase64: reply.payloadBase64, timestampMs: reply.timestampMs };
      case "failed":
        if (typeof reply.code !== "string") break;
        return { state: "failed", code: reply.code };
      case "resolve_delay":
        if (typeof reply.attempt !== "number" || !Number.isInteger(reply.attempt)
            || reply.attempt < 1 || reply.attempt > 101 || typeof reply.code !== "string") break;
        assertConfig(reply.config);
        return { state: "resolve_delay", attempt: reply.attempt, code: reply.code, config: reply.config };
      case "suspended":
        this.#yield = true;
        return { state: "suspended" };
    }
    this.#unknown = true;
    throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
  }

  async claimBatch(body: { steps: WorkflowClaimDeclaration[] }): Promise<WorkflowBatchReply> {
    if (this.#drainIncomplete) return { state: "suspended" };
    if (this.#yield) return { state: "suspended" };
    this.#claiming = true;
    try {
      const reply = await this.#request("claim-batch", { ...body,
        remainingMs: Math.max(0, Math.floor(this.#budgetMs - (performance.now() - this.#startedAt))) });
      if (typeof reply.errorCode === "string" && reply.errorCode) return { errorCode: reply.errorCode };
      if (reply.state === "suspended") { this.#yield = true; return { state: "suspended" }; }
      if (!Array.isArray(reply?.steps) || reply.steps.length < 1 || reply.steps.length > 16
          || reply.steps.length !== body?.steps?.length) throw new Error("invalid batch");
      this.#grants.clear();
      const steps: WorkflowBatchStep[] = [];
      for (let i = 0; i < reply.steps.length; i++) {
        const step: unknown = reply.steps[i];
        if (!record(step)) throw new Error("invalid step");
        const ordinal = body.steps[i]!.ordinal;
        if (step.state !== "run") {
          // Large replay values are fetched individually, not in claim-batch.
          if (step.state === "resolve_delay") {
            if (typeof step.attempt !== "number" || !Number.isInteger(step.attempt)
                || step.attempt < 1 || step.attempt > 101 || typeof step.code !== "string") {
              throw new Error("invalid delay grant");
            }
            assertConfig(step.config);
            steps.push({ ordinal, state: "resolve_delay", attempt: step.attempt, code: step.code, config: step.config });
            continue;
          }
          if (step.state !== "complete" && step.state !== "failed" && step.state !== "suspended"
              && step.state !== "rollback_boundary") throw new Error("invalid state");
          if (step.state === "suspended") this.#yield = true;
          if (step.state === "rollback_boundary") {
            if (typeof step.rollbackOrdinal !== "number" || !Number.isInteger(step.rollbackOrdinal)
                || step.rollbackOrdinal < 0 || step.rollbackOrdinal > 1024) throw new Error("invalid rollback frontier");
            steps.push({ ordinal, state: "rollback_boundary", rollbackOrdinal: step.rollbackOrdinal });
          } else if (step.state === "complete" && (step.attempt !== undefined || step.config !== undefined)) {
            if (typeof step.attempt !== "number" || !Number.isInteger(step.attempt)
                || step.attempt < 1 || step.attempt > 101) throw new Error("invalid replay attempt");
            assertConfig(step.config);
            steps.push({ ordinal, state: "complete", attempt: step.attempt, config: step.config });
          } else {
            steps.push({ ordinal, state: step.state });
          }
          continue;
        }
        if (typeof step.stepToken !== "string" || !/^[0-9a-f]{64}$/.test(step.stepToken)
            || typeof step.attempt !== "number" || !Number.isInteger(step.attempt) || step.attempt < 1 || step.attempt > 101
            || typeof step.remainingMs !== "number" || !Number.isSafeInteger(step.remainingMs) || step.remainingMs < 0 || step.remainingMs > 240000) {
          throw new Error("invalid grant");
        }
        assertConfig(step.config);
        let resolve!: (value: WorkflowVerdict) => void;
        const result = new Promise<WorkflowVerdict>(done => { resolve = done; });
        let acknowledge!: () => void;
        const acknowledgment = new Promise<void>(done => { acknowledge = done; });
        const grant: Grant = { stepToken: step.stepToken, attempt: step.attempt,
          result, resolve, acknowledgment, acknowledge, acknowledged: false,
          settled: false, committing: false, timer: null };
        this.#grants.set(ordinal, grant);
        grant.timer = setTimeout(() => {
          this.#commit("timeout", { ordinal }).catch(() => {
            grant.resolve({ errorCode: "WORKFLOW_RUNTIME_UNAVAILABLE" });
          });
        }, step.remainingMs);
        steps.push({ ordinal, state: "run", attempt: step.attempt, config: step.config });
      }
      return { steps };
    } catch {
      this.#unknown = true;
      throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    } finally { this.#claiming = false; }
  }

  async #commit(operation: string, body: {
    ordinal: number; code?: string; outputBase64?: string; resolvedDelayMs?: number;
  }): Promise<WorkflowVerdict> {
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
        ...body, stepToken: grant.stepToken, attempt: grant.attempt,
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

  success(body: { ordinal: number; outputBase64: string }) { return this.#commit("success", body); }
  failure(body: { ordinal: number; code: string; resolvedDelayMs?: number }) { return this.#commit("failure", body); }
  resolveDelay(body: { ordinal: number; attempt: number; code: string; resolvedDelayMs?: number }) {
    return this.#request("resolve-delay", body).then(reply => this.#verdict(reply));
  }

  async result(ordinal: number): Promise<WorkflowVerdict> {
    const grant = this.#grants.get(ordinal);
    if (grant) return grant.result;
    return this.#verdict(await this.#request("result", { ordinal }));
  }

  async drain(): Promise<WorkflowDrainReply> {
    const pending = [...this.#grants.values()].filter(grant => !grant.acknowledged);
    if (pending.length !== 0) {
      let timer: number | null = null;
      // A timeout fences commits, not arbitrary JS or external side effects. Wait
      // a bounded margin for actual callback reports, then quarantine the run.
      const acknowledged = await Promise.race([
        Promise.all(pending.map(grant => grant.acknowledgment)).then(() => true),
        new Promise<boolean>(resolve => { timer = setTimeout(() => resolve(false), 30000); }),
      ]);
      clearTimeout(timer);
      if (!acknowledged) {
        this.#drainIncomplete = true;
        return { state: "suspended" };
      }
    }
    // Do not retain a full batch's serialized results while reading an event or
    // serializing the final output. SQLite remains the replay authority.
    if ([...this.#grants.values()].every(grant => grant.settled)) this.#grants.clear();
    return { ok: true };
  }

  // Called only by the trusted host after the loaded RPC finishes. A forged or
  // caught tenant signal is never authority to yield or to commit success.
  async [finishWorkflowRun](result: WorkflowRunResult) {
    if (this.#drainIncomplete) {
      return { result: { outcome: "unknown", finalOrdinal: result.finalOrdinal }, drainIncomplete: true };
    }
    if (this.#unknown || this.#closed || this.#claiming
        || [...this.#grants.values()].some(grant => !grant.settled)) {
      throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    if (this.#yield) {
      const reply = await this.#request("yield", { finalOrdinal: result.finalOrdinal });
      if (reply?.ok !== true) throw new Error("WORKFLOW_RUNTIME_UNAVAILABLE");
      return { result: { outcome: "suspended", finalOrdinal: result.finalOrdinal }, drainIncomplete: false };
    }
    if (!["complete", "errored", "terminated"].includes(result?.outcome)) throw new Error("invalid outcome");
    return { result, drainIncomplete: false };
  }
}
