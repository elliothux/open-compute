import { WorkflowEntrypoint } from "cloudflare:workers";
import { NonRetryableError } from "cloudflare:workflows";
import {
  decodeWorkflowBase64,
  encodeWorkflowBase64,
  workflowError,
  workflowString,
  workflowSerializationCode,
} from "./codec.js";
import { durationMs } from "./duration.js";
import type { WorkflowCallback, WorkflowClass, WorkflowController, WorkflowDeclaration, WorkflowEventWire, WorkflowProtocolError, WorkflowRunResult, WorkflowVerdict } from "./execution-protocol.js";

interface PendingStep {
  descriptor: WorkflowDeclaration;
  callback: WorkflowCallback | undefined;
  dynamicDelay: WorkflowDelayFunction | undefined;
  rollback: RollbackRegistration | undefined;
  observed: boolean;
  resolve(value: unknown): void;
  reject(reason: unknown): void;
}

type WorkflowDelayFunction = (input: { ctx: unknown; error: Error }) => unknown;
type RollbackFunction = (input: { ctx: unknown; error: Error; output: unknown; stepName: string }) => unknown;
interface RollbackRegistration {
  ordinal: number;
  name: string;
  callback: RollbackFunction;
  config: unknown;
  dynamicDelay: WorkflowDelayFunction | undefined;
  context?: unknown;
  output?: unknown;
}

class DeferredWorkflowPromise<T> implements Promise<T> {
  readonly [Symbol.toStringTag] = "Promise";
  readonly #promise: Promise<T>;
  readonly #observe: () => void;
  constructor(promise: Promise<T>, observe: () => void) {
    this.#promise = promise;
    this.#observe = observe;
  }
  then<TResult1 = T, TResult2 = never>(
    onfulfilled?: ((value: T) => TResult1 | PromiseLike<TResult1>) | null,
    onrejected?: ((reason: unknown) => TResult2 | PromiseLike<TResult2>) | null,
  ): Promise<TResult1 | TResult2> {
    this.#observe();
    return this.#promise.then(onfulfilled, onrejected);
  }
  catch<TResult = never>(
    onrejected?: ((reason: unknown) => TResult | PromiseLike<TResult>) | null,
  ): Promise<T | TResult> {
    this.#observe();
    return this.#promise.catch(onrejected);
  }
  finally(onfinally?: (() => void) | null): Promise<T> {
    this.#observe();
    return this.#promise.finally(onfinally);
  }
}

function isCallback(value: unknown): value is WorkflowCallback { return typeof value === "function"; }
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function doConfig(value: unknown): { config: unknown; dynamicDelay: WorkflowDelayFunction | undefined } {
  if (!object(value) || Object.keys(value).some(key => !["retries", "timeout", "sensitive"].includes(key))) {
    throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
  }
  const retries = value.retries;
  if (retries !== undefined) {
    if (!object(retries) || Object.keys(retries).some(key => !["limit", "delay", "backoff"].includes(key))) {
      throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
    }
    if (typeof retries.delay === "function") {
      const normalizedRetries = { ...retries, delay: { dynamic: true } };
      return { config: { ...value, retries: normalizedRetries }, dynamicDelay: retries.delay as WorkflowDelayFunction };
    }
  }
  return { config: value, dynamicDelay: undefined };
}

function rollbackOptions(value: unknown): {
  callback: RollbackFunction; config: unknown; dynamicDelay: WorkflowDelayFunction | undefined;
} | undefined {
  if (value === undefined) return undefined;
  if (!object(value) || Object.keys(value).some(key => !["rollback", "rollbackConfig"].includes(key))
      || typeof value.rollback !== "function") {
    throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
  }
  const normalized = doConfig(value.rollbackConfig ?? {});
  if (object(normalized.config) && "sensitive" in normalized.config) {
    throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
  }
  return {
    callback: value.rollback as RollbackFunction,
    config: normalized.config,
    dynamicDelay: normalized.dynamicDelay,
  };
}

export function validateWorkflowClass(target: unknown): target is WorkflowClass {
  return typeof target === "function"
    && WorkflowEntrypoint.prototype.isPrototypeOf(target.prototype)
    && typeof target.prototype.run === "function";
}

// The system controller owns timeout and all grants. The local marker only
// unwinds cooperative run() code; catching it cannot reopen the controller.
export async function runWorkflow(target: unknown, ctx: ExecutionContext, env: Record<string, unknown>, event: WorkflowEventWire, backend: WorkflowController): Promise<WorkflowRunResult> {
  if (!validateWorkflowClass(target)) throw workflowError("WORKFLOW_VERSION_NOT_READY");
  const suspension = Object.freeze(Object.create(null));
  const rollbackBoundary = Object.freeze(Object.create(null));
  const rollbackTrigger = new Error("Instance terminated during rollback");
  let ordinal = 0;
  let closed = false;
  let suspended = false;
  let unknown = false;
  let failure: string | undefined;
  let rollingBack = false;
  let rollbackOrdinal: number | undefined;
  let collecting: PendingStep[] = [];
  let flushScheduled = false;
  let frontier: number[] = [];
  const counts = new Map<string, number>();
  const pending = new Set<Promise<void>>();
  const background: Promise<unknown>[] = [];
  const settledFailures = new WeakMap<object, string>();
  const rollbacks = new Map<number, RollbackRegistration>();
  const rememberFailure = settledFailures.set.bind(settledFailures);
  const recalledFailure = settledFailures.get.bind(settledFailures);
  const reject: (code: string) => never = code => { failure ||= code; throw workflowError(code); };
  const check = () => {
    if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
    if (rollingBack) throw rollbackBoundary;
    if (suspended) throw suspension;
    if (closed) reject("WORKFLOW_RUN_STALE");
  };
  const rpc = async <T extends { errorCode?: string | undefined }>(operation: () => Promise<T>): Promise<Exclude<T, WorkflowProtocolError>> => {
    let reply: T;
    try { reply = await operation(); }
    catch { unknown = true; throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
    if (reply?.errorCode) reject(reply.errorCode);
    return reply as Exclude<T, WorkflowProtocolError>;
  };
  const report = async (operation: () => Promise<WorkflowVerdict>): Promise<void> => {
    // A callback reporting after the trusted timeout may only acknowledge its
    // drain. The independent result RPC carries the durable verdict.
    let reply;
    try { reply = await operation(); }
    catch { unknown = true; throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
    if (reply?.errorCode && reply.errorCode !== "WORKFLOW_STEP_STALE") reject(reply.errorCode);
  };
  const value = (reply: Exclude<WorkflowVerdict, WorkflowProtocolError>): unknown => {
    if (reply.state === "suspended") { suspended = true; throw suspension; }
    if (reply.state === "failed") {
      if (!["WORKFLOW_STEP_TIMEOUT", "WORKFLOW_STEP_RETRIES_EXHAUSTED", "WORKFLOW_NON_RETRYABLE", "WORKFLOW_EVENT_TIMEOUT"].includes(reply.code)) {
        reject(reply.code);
      }
      const error = reply.code === "WORKFLOW_NON_RETRYABLE"
        ? new NonRetryableError("Workflow step is not retryable") : workflowError(reply.code);
      error.stack = `${error.name}: ${error.message}`;
      rememberFailure(error, reply.code);
      throw error;
    }
    if (reply.state === "event") return reply;
    if (reply.state !== "complete") {
      unknown = true;
      throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
    try { return reply.outputBase64 === undefined ? undefined : decodeWorkflowBase64(reply.outputBase64); }
    catch {
      unknown = true;
      throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
    }
  };
  const callbackContext = (item: PendingStep, attempt: number, config: {
    retries: { limit: number; delay?: number; backoff: "constant" | "linear" | "exponential" };
    timeout: number; sensitive?: "output";
  }) => Object.freeze({
    step: Object.freeze({ name: item.descriptor.name, count: item.descriptor.nameCount }),
    attempt,
    config: Object.freeze({
      retries: Object.freeze({
        limit: config.retries.limit,
        backoff: config.retries.backoff,
        ...(config.retries.delay === undefined ? {} : { delay: config.retries.delay }),
      }),
      timeout: config.timeout,
      ...(config.sensitive === undefined ? {} : { sensitive: config.sensitive }),
    }),
  });
  const replayConfig = (raw: unknown) => {
    if (!object(raw)) throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
    const retries = object(raw.retries) ? raw.retries : {};
    const delay = retries.delay === undefined ? 10_000
      : object(retries.delay) && retries.delay.dynamic === true ? undefined : durationMs(retries.delay);
    return {
      retries: {
        limit: retries.limit === undefined ? 5 : retries.limit as number,
        ...(delay === undefined ? {} : { delay }),
        backoff: (retries.backoff ?? "exponential") as "constant" | "linear" | "exponential",
      },
      timeout: raw.timeout === undefined ? 60_000 : durationMs(raw.timeout),
      ...(raw.sensitive === undefined ? {} : { sensitive: raw.sensitive as "output" }),
    };
  };
  const registerRollback = (item: PendingStep, attempt: number,
    config: ReturnType<typeof replayConfig>, output: unknown) => {
    const rollback = item.rollback;
    if (!rollback) return;
    rollback.context = callbackContext(item, attempt, config);
    rollback.output = output;
    rollbacks.set(rollback.ordinal, rollback);
  };
  const evaluateDynamicDelay = async (item: PendingStep, attempt: number,
    config: { retries: { limit: number; delay?: number; backoff: "constant" | "linear" | "exponential" };
      timeout: number; sensitive?: "output" }, error: Error): Promise<number> => {
    if (!item.dynamicDelay || config.retries.delay !== undefined) {
      throw workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
    }
    let timer: number | undefined;
    try {
      const returned = await Promise.race([
        item.dynamicDelay(Object.freeze({ ctx: callbackContext(item, attempt, config), error })),
        new Promise<never>((_resolve, rejectDelay) => {
          timer = setTimeout(() => rejectDelay(workflowError("WORKFLOW_STEP_CONFIG_UNSUPPORTED")), 5_000);
        }),
      ]);
      return durationMs(returned);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  };
  const resolveDynamicDelay = async (item: PendingStep, grant: {
    state: "resolve_delay"; attempt: number; code: string;
    config: { retries: { limit: number; delay?: number; backoff: "constant" | "linear" | "exponential" };
      timeout: number; sensitive?: "output" };
  }): Promise<Exclude<WorkflowVerdict, WorkflowProtocolError>> => {
    let code = grant.code;
    let resolvedDelayMs: number | undefined;
    try {
      resolvedDelayMs = await evaluateDynamicDelay(
        item,
        grant.attempt,
        grant.config,
        workflowError(grant.code),
      );
    } catch {
      code = "WORKFLOW_STEP_CONFIG_UNSUPPORTED";
      failure ||= code;
    }
    return rpc(() => backend.resolveDelay({
      ordinal: item.descriptor.ordinal,
      attempt: grant.attempt,
      code,
      ...(resolvedDelayMs === undefined ? {} : { resolvedDelayMs }),
    }));
  };
  const descriptor = (kind: WorkflowDeclaration["kind"], name: string, config: unknown,
    rollbackConfig?: unknown, internal = false): WorkflowDeclaration => {
    if (internal) {
      if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
      if (suspended) throw suspension;
      if (closed) reject("WORKFLOW_RUN_STALE");
    } else check();
    try { workflowString(name, 256, "WORKFLOW_SERIALIZATION_UNSUPPORTED"); }
    catch { reject("WORKFLOW_SERIALIZATION_UNSUPPORTED"); }
    if (!name) reject("WORKFLOW_SERIALIZATION_UNSUPPORTED");
    const key = `${kind}/${name}`;
    const nameCount = (counts.get(key) || 0) + 1;
    counts.set(key, nameCount);
    return { ordinal: ordinal++, kind, name, nameCount, config,
      ...(rollbackConfig === undefined ? {} : { rollbackConfig }), rollbackStep: internal,
      dependencies: [...frontier] };
  };
  const execute = async (batch: PendingStep[]): Promise<void> => {
    try {
      const reply = await rpc(() => backend.claimBatch({ steps: batch.map(item => ({
        ...item.descriptor, batchFirstOrdinal: batch[0]!.descriptor.ordinal, batchSize: batch.length,
      })) }));
      if (reply.state === "suspended") { suspended = true; throw suspension; }
      if (!Array.isArray(reply.steps) || reply.steps.length !== batch.length) {
        unknown = true;
        throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
      }
      const outcomes = await Promise.all(batch.map(async (item, index) => {
        const grant = reply.steps[index]!;
        const indexOrdinal = item.descriptor.ordinal;
        if (grant.state === "rollback_boundary") {
          rollingBack = true;
          rollbackOrdinal = grant.rollbackOrdinal;
          return { ok: false as const, error: rollbackBoundary };
        }
        if (grant.state === "resolve_delay") {
          const verdict = await resolveDynamicDelay(item, grant);
          return { ok: true as const, value: value(verdict as Exclude<WorkflowVerdict, WorkflowProtocolError>) };
        }
        if (grant.state === "run") {
          if (item.descriptor.kind !== "do" || !item.callback) {
            unknown = true;
            throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
          }
          const callbackFunction = item.callback;
          // This task is observed but is not the timeout authority. Its late
          // report is rejected by the controller and the durable step fence.
          const callback = (async () => {
            let result;
            const context = callbackContext(item, grant.attempt, grant.config);
            try {
              result = await callbackFunction(context);
            } catch (error) {
              // Do not read error.message/name/stack/cause or invoke getters.
              let code = failure || "WORKFLOW_EXECUTION_FAILED";
              try { if (!failure && error instanceof NonRetryableError) code = "WORKFLOW_NON_RETRYABLE"; } catch { /* hostile proxy */ }
              let resolvedDelayMs: number | undefined;
              if (code === "WORKFLOW_EXECUTION_FAILED" && item.dynamicDelay
                  && grant.attempt <= grant.config.retries.limit) {
                let callbackError = new Error("Workflow step failed");
                try { if (error instanceof Error) callbackError = error; } catch { /* hostile proxy */ }
                try {
                  resolvedDelayMs = await evaluateDynamicDelay(item, grant.attempt, grant.config, callbackError);
                } catch {
                  code = "WORKFLOW_STEP_CONFIG_UNSUPPORTED";
                  failure ||= code;
                }
              }
              await report(() => backend.failure({
                ordinal: indexOrdinal,
                code,
                ...(resolvedDelayMs === undefined ? {} : { resolvedDelayMs }),
              }));
              return;
            }
            let outputBase64;
            try { outputBase64 = encodeWorkflowBase64(result); }
            catch (error) {
              const code = workflowSerializationCode(error);
              failure ||= code;
              await report(() => backend.failure({ ordinal: indexOrdinal, code: failure || code }));
              return;
            }
            await report(() => backend.success({ ordinal: indexOrdinal, outputBase64 }));
          })();
          callback.catch(() => {});
        }
        try {
          let reply = await rpc(() => backend.result(indexOrdinal));
          if (reply.state === "resolve_delay") reply = await resolveDynamicDelay(item, reply);
          if (reply.state === "suspended" || reply.state === "failed") {
            return { ok: true as const, value: value(reply) };
          }
          if (item.descriptor.kind === "wait_event") {
            if (reply.state !== "event") {
              unknown = true;
              throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
            }
            let payload;
            try { payload = decodeWorkflowBase64(reply.payloadBase64); }
            catch { unknown = true; throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE"); }
            return { ok: true as const, value: Object.freeze({
              type: reply.type, payload, timestamp: new Date(reply.timestampMs),
            }) };
          }
          if (reply.state === "event") {
            unknown = true;
            throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
          }
          const resolved = value(reply);
          if (item.descriptor.kind === "do") {
            const replay = grant.state === "complete" && grant.config !== undefined
              ? grant.config : replayConfig(item.descriptor.config);
            const attempt = grant.state === "complete" && grant.attempt !== undefined ? grant.attempt : 1;
            registerRollback(item, attempt, replay, resolved);
          }
          return { ok: true as const, value: resolved };
        }
        catch (error) { return { ok: false as const, error }; }
      }));
      const drain = await rpc(() => backend.drain());
      if (drain.state === "suspended") suspended = true;
      for (let i = 0; i < batch.length; i++) {
        const outcome = outcomes[i]!;
        if (suspended) batch[i]!.reject(suspension);
        else if (outcome.ok) batch[i]!.resolve(outcome.value);
        else batch[i]!.reject(outcome.error);
      }
    } catch (error) {
      for (const item of batch) item.reject(error);
    }
  };
  const flush = () => {
    flushScheduled = false;
    if (collecting.length === 0) return;
    const batch = collecting;
    collecting = [];
    if (batch.some(item => !item.observed)) {
      failure ||= "WORKFLOW_NON_DETERMINISTIC";
      const error = workflowError("WORKFLOW_NON_DETERMINISTIC");
      for (const item of batch) item.reject(error);
      return;
    }
    frontier = batch.map(item => item.descriptor.ordinal);
    const task = execute(batch);
    pending.add(task);
    task.finally(() => pending.delete(task));
  };
  const observe = () => {
    if (flushScheduled || collecting.length === 0) return;
    flushScheduled = true;
    // Consumption drives the durable group boundary. Promise.all registers all
    // children before this microtask, while unrelated microtask timing no longer
    // decides whether declarations are parallel.
    queueMicrotask(flush);
  };
  const enqueue = (kind: WorkflowDeclaration["kind"], name: string, config: unknown,
    callback?: WorkflowCallback, dynamicDelay?: WorkflowDelayFunction,
    rollback?: RollbackRegistration, internal = false) => {
    const declaration = descriptor(kind, name, config, rollback?.config, internal);
    if (rollback) rollback.ordinal = declaration.ordinal;
    let resolve!: (value: unknown) => void;
    let rejectResult!: (reason: unknown) => void;
    const base = new Promise<unknown>((yes, no) => { resolve = yes; rejectResult = no; });
    base.catch(() => {});
    const item = {
      descriptor: declaration,
      callback,
      dynamicDelay,
      rollback,
      observed: false,
      resolve,
      reject: rejectResult,
    };
    collecting.push(item);
    return new DeferredWorkflowPromise(base, () => {
      item.observed = true;
      observe();
    });
  };
  const step = Object.freeze({
    do(name: string, config: unknown, callbackOrRollback?: WorkflowCallback | unknown,
      rollbackArgument?: unknown, ...extra: unknown[]): Promise<unknown> {
      let callback: WorkflowCallback | undefined;
      let rollbackValue: unknown;
      if (isCallback(config)) {
        callback = config;
        config = {};
        rollbackValue = callbackOrRollback;
        if (rollbackArgument !== undefined) extra.push(rollbackArgument);
      } else {
        callback = isCallback(callbackOrRollback) ? callbackOrRollback : undefined;
        rollbackValue = rollbackArgument;
      }
      if (!callback || extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
      let normalized;
      let rollback;
      try {
        normalized = doConfig(config);
        const options = rollbackOptions(rollbackValue);
        rollback = options && {
          ordinal: 0, name, callback: options.callback, config: options.config,
          dynamicDelay: options.dynamicDelay,
        };
      }
      catch { reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED"); }
      return enqueue("do", name, normalized.config, callback, normalized.dynamicDelay, rollback);
    },
    sleep(name: string, duration: unknown, ...extra: unknown[]) {
      if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
      return enqueue("sleep", name, { duration });
    },
    sleepUntil(name: string, timestamp: unknown, ...extra: unknown[]) {
      if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
      return enqueue("sleep_until", name, { timestamp: timestamp instanceof Date ? timestamp.getTime() : timestamp });
    },
    waitForEvent(name: string, options: unknown, ...extra: unknown[]) {
      if (extra.length) reject("WORKFLOW_STEP_CONFIG_UNSUPPORTED");
      return enqueue("wait_event", name, options);
    },
  });
  const executeRollbacks = async (): Promise<void> => {
    rollingBack = true;
    ordinal = rollbackOrdinal ?? ordinal;
    frontier = [];
    const registrations = [...rollbacks.values()].sort((left, right) => right.ordinal - left.ordinal);
    for (const registration of registrations) {
      try {
        await enqueue(
          "do",
          `rollback:${registration.ordinal}`,
          registration.config,
          async () => registration.callback(Object.freeze({
            ctx: registration.context,
            error: rollbackTrigger,
            output: registration.output,
            stepName: registration.name,
          })),
          registration.dynamicDelay,
          undefined,
          true,
        );
      } catch {
        // Cloudflare stops the LIFO chain at the first exhausted rollback. The
        // external terminate still reaches the durable Terminated state.
        break;
      }
    }
    rollbacks.clear();
  };
  Object.defineProperty(ctx, "waitUntil", { value(promise: Promise<unknown>) {
    if (closed || suspended) reject("WORKFLOW_RUN_STALE");
    const observed = Promise.resolve(promise);
    observed.catch(() => {});
    background.push(observed);
  } });
  let outputBase64;
  try {
    const instance = new target(ctx, env);
    const result = await instance.run(Object.freeze({
      payload: decodeWorkflowBase64(event.payloadBase64), timestamp: new Date(event.createdAtMs),
      instanceId: event.externalInstanceId, workflowName: event.definitionName,
      ...(event.schedule === undefined ? {} : { schedule: Object.freeze({ ...event.schedule }) }),
    }), step);
    flush();
    await Promise.all([...pending]);
    for (let index = 0; index < background.length; index++) await background[index];
    flush();
    await Promise.all([...pending]);
    try { outputBase64 = encodeWorkflowBase64(result); }
    catch (error) { failure ||= workflowSerializationCode(error); }
  } catch (error) {
    if (error !== suspension && error !== rollbackBoundary) {
      const code = error !== null && (typeof error === "object" || typeof error === "function") ? recalledFailure(error) : undefined;
      failure ||= code || "WORKFLOW_EXECUTION_FAILED";
    }
  }
  // Sibling commits must finish before terminal/yield; tenant Promise.all's
  // first rejection cannot discard the other callbacks' durable results.
  flush();
  await Promise.all([...pending]);
  if (event.rollback && !suspended) {
    const replayFailure = failure;
    if (replayFailure === undefined || ["WORKFLOW_STEP_TIMEOUT", "WORKFLOW_STEP_RETRIES_EXHAUSTED",
      "WORKFLOW_NON_RETRYABLE", "WORKFLOW_EVENT_TIMEOUT", "WORKFLOW_EXECUTION_FAILED"].includes(replayFailure)) {
      failure = undefined;
      await executeRollbacks();
      flush();
      await Promise.all([...pending]);
      closed = true;
      if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
      return { outcome: "terminated", finalOrdinal: ordinal };
    }
  }
  closed = true;
  if (unknown) throw workflowError("WORKFLOW_RUNTIME_UNAVAILABLE");
  return suspended
    ? { outcome: "suspended", finalOrdinal: ordinal }
    : failure ? { outcome: "errored", errorCode: failure, finalOrdinal: ordinal }
      : { outcome: "complete", outputBase64: outputBase64!, finalOrdinal: ordinal };
}
