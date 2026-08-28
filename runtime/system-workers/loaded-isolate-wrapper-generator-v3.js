// Revision three supports independent legacy and durable Workflow caller bindings.
// Earlier revisions remain immutable inputs to persisted deployment digests.
export const R2_FACADE_MODULE = "__open_compute_r2_facade__.js";
export const D1_FACADE_MODULE = "__open_compute_d1_facade__.js";
export const DO_FACADE_MODULE = "__open_compute_do_facade__.js";
export const DO_ID_CODEC_MODULE = "__open_compute_do_id_codec__.js";
export const DO_ALARM_SHIM_MODULE = "__open_compute_do_alarm_shim__.js";
export const QUEUE_FACADE_MODULE = "__open_compute_queue_facade__.js";
export const WORKFLOW_RUNNER_MODULE = "__open_compute_workflow_runner__.js";
export const WORKFLOW_JSON_MODULE = "__open_compute_workflow_json__.js";
export const WORKFLOW_V2_FACADE_MODULE = "__open_compute_workflow_facade_v2__.js";
export const WORKFLOW_FACADE_MODULE = "__open_compute_workflow_facade__.js";
export const LOADED_ISOLATE_WRAPPER_MODULE = "__open_compute_loaded_isolate_wrapper__.js";
export const LOADED_ISOLATE_RESERVED_MODULES = Object.freeze([
  R2_FACADE_MODULE,
  D1_FACADE_MODULE,
  DO_FACADE_MODULE,
  DO_ID_CODEC_MODULE,
  DO_ALARM_SHIM_MODULE,
  QUEUE_FACADE_MODULE,
  WORKFLOW_RUNNER_MODULE,
  WORKFLOW_JSON_MODULE,
  WORKFLOW_FACADE_MODULE,
  WORKFLOW_V2_FACADE_MODULE,
  LOADED_ISOLATE_WRAPPER_MODULE,
]);

export function generateBindingWrapper(
  mainModule,
  r2BindingNames,
  d1BindingNames,
  doBindingNames,
  entrypointName,
  durableObject,
  queueBindingNames = [],
  workflow = false,
  workflowBindingNames = [],
  workflowRunnerSource = "",
  workflowV2BindingNames = [],
) {
  const main = JSON.stringify(`./${mainModule}`);
  const r2Bindings = JSON.stringify(r2BindingNames);
  const d1Bindings = JSON.stringify(d1BindingNames);
  const doBindings = JSON.stringify(doBindingNames);
  const queueBindings = JSON.stringify(queueBindingNames);
  const workflowBindings = JSON.stringify(workflowBindingNames);
  const workflowV2Bindings = JSON.stringify(workflowV2BindingNames);
  const imports = [
    r2BindingNames.length ? `import { R2Bucket } from "./${R2_FACADE_MODULE}";` : "",
    d1BindingNames.length ? `import { D1Database } from "./${D1_FACADE_MODULE}";` : "",
    doBindingNames.length ? `import { DurableObjectNamespace } from "./${DO_FACADE_MODULE}";` : "",
    queueBindingNames.length ? `import { QueueProducer } from "./${QUEUE_FACADE_MODULE}";` : "",
    workflowBindingNames.length ? `import { WorkflowBinding } from "./${WORKFLOW_FACADE_MODULE}";` : "",
    workflowV2BindingNames.length ? `import { WorkflowBindingV2 } from "./${WORKFLOW_V2_FACADE_MODULE}";` : "",
    workflow ? workflowRunnerSource.replace(/^export /gm, "") : "",
  ].join("\n");
  const wraps = [
    r2BindingNames.length ? "for (const name of R2_BINDINGS) out[name] = new R2Bucket(out[name]);" : "",
    d1BindingNames.length ? "for (const name of D1_BINDINGS) out[name] = new D1Database(out[name]);" : "",
    doBindingNames.length ? "for (const name of DO_BINDINGS) out[name] = new DurableObjectNamespace(out[name]);" : "",
    queueBindingNames.length
      ? `for (const name of QUEUE_BINDINGS) out[name] = new QueueProducer(out[name], ${durableObject === true});`
      : "",
    workflowBindingNames.length ? `for (const name of WORKFLOW_BINDINGS) out[name] = new WorkflowBinding(out[name], ${durableObject === true});` : "",
    workflowV2BindingNames.length ? `for (const name of WORKFLOW_V2_BINDINGS) out[name] = new WorkflowBindingV2(out[name], ${durableObject === true});` : "",
  ].join("\n");
  const doContext = entrypointName && durableObject ? `
import {
  activateDurableObjectAlarm,
  dispatchDurableObjectAlarm,
  prepareDurableObjectContext,
  repairDurableObjectAlarm,
} from "./${DO_ALARM_SHIM_MODULE}";
const PRIVATE_ALARM_INDEX = "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX";
const durableObjectAlarmState = new WeakMap();
` : "";
  const named = workflow ? `
export class __OpenComputeWorkflow extends WorkerEntrypoint {
  validate() { return validateWorkflowClass(tenant[${JSON.stringify(entrypointName)}]); }
  execute(event, backend) {
    const wrapped = wrapEnv(this.env);
    return withEnv(wrapped, () => runWorkflow(
      tenant[${JSON.stringify(entrypointName)}], this.ctx, wrapped, event, backend,
    ));
  }
}
` : entrypointName && durableObject ? `
if (typeof tenant[${JSON.stringify(entrypointName)}] !== "function") throw new Error("missing entrypoint");
const NamedWrapped = ({
  [${JSON.stringify(entrypointName)}]: class extends tenant[${JSON.stringify(entrypointName)}] {
    constructor(ctx, env) {
      const wrapped = wrapEnv(env);
      const prepared = prepareDurableObjectContext(ctx, env[PRIVATE_ALARM_INDEX]);
      withEnv(wrapped, () => super(prepared.context, wrapped));
      durableObjectAlarmState.set(this, prepared);
      activateDurableObjectAlarm(prepared);
      return wrapInstance(this, wrapped);
    }
    async __openComputeAlarm(payload) {
      return dispatchDurableObjectAlarm(
        this,
        tenant[${JSON.stringify(entrypointName)}].prototype.alarm,
        durableObjectAlarmState.get(this),
        payload,
      );
    }
    async __openComputeAlarmRepair() {
      return repairDurableObjectAlarm(durableObjectAlarmState.get(this));
    }
  }
})[${JSON.stringify(entrypointName)}];
export { NamedWrapped as ${entrypointName} };
` : entrypointName ? `
if (typeof tenant[${JSON.stringify(entrypointName)}] !== "function") throw new Error("missing entrypoint");
const NamedWrapped = ({
  [${JSON.stringify(entrypointName)}]: class extends tenant[${JSON.stringify(entrypointName)}] {
    constructor(ctx, env) {
      const wrapped = wrapEnv(env);
      withEnv(wrapped, () => super(ctx, wrapped));
      return wrapInstance(this, wrapped);
    }
  }
})[${JSON.stringify(entrypointName)}];
export { NamedWrapped as ${entrypointName} };
` : "";
  return `
import { withEnv, WorkerEntrypoint } from "cloudflare:workers";
${imports}
import * as tenant from ${main};
export * from ${main};
${doContext}
const R2_BINDINGS = ${r2Bindings};
const D1_BINDINGS = ${d1Bindings};
const DO_BINDINGS = ${doBindings};
const QUEUE_BINDINGS = ${queueBindings};
const WORKFLOW_BINDINGS = ${workflowBindings};
const WORKFLOW_V2_BINDINGS = ${workflowV2Bindings};
const wrappedMarker = Symbol("open-compute.loaded-isolate-wrapped-env");
function wrapEnv(env) {
  if (!env || env[wrappedMarker]) return env;
  const out = {};
  for (const [key, value] of Object.entries(env)) {
    if (key !== "__OPEN_COMPUTE_PRIVATE_ALARM_INDEX") out[key] = value;
  }
  ${wraps}
  Object.defineProperty(out, wrappedMarker, { value: true });
  return out;
}
function invoke(owner, fn, args, env) {
  return withEnv(env, () => Reflect.apply(fn, owner, args));
}
function wrapInstance(instance, env) {
  return new Proxy(instance, {
    get(target, property) {
      const value = Reflect.get(target, property, target);
      return typeof value === "function" ? (...args) => invoke(target, value, args, env) : value;
    }
  });
}
function normalizedEvent(kind, event) {
  if (kind === "scheduled" && event && event.type === undefined) {
    return new Proxy(event, {
      get(target, property) {
        if (property === "type") return "scheduled";
        const value = Reflect.get(target, property, target);
        return typeof value === "function" ? value.bind(target) : value;
      }
    });
  }
  return event;
}
function wrapHandler(owner, fn, kind) {
  return function(event, env, ctx) {
    const wrapped = wrapEnv(env);
    return invoke(owner, fn, [normalizedEvent(kind, event), wrapped, ctx], wrapped);
  };
}
const raw = tenant.default;
let wrappedDefault = raw;
if (raw && typeof raw === "object") {
  wrappedDefault = { ...raw };
  for (const key of ["fetch", "scheduled", "queue", "tail"]) {
    if (typeof raw[key] === "function") wrappedDefault[key] = wrapHandler(raw, raw[key], key);
  }
} else if (typeof raw === "function") {
  if (/^\\s*class\\b/.test(Function.prototype.toString.call(raw))) {
    wrappedDefault = class extends raw {
      constructor(ctx, env) {
        const wrapped = wrapEnv(env);
        withEnv(wrapped, () => super(ctx, wrapped));
        return wrapInstance(this, wrapped);
      }
    };
  } else {
    wrappedDefault = { fetch: wrapHandler(undefined, raw, "fetch") };
  }
}
${named}
export default wrappedDefault;
`;
}
