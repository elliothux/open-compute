import {
  activateDurableObjectAlarm, dispatchDurableObjectAlarm,
  prepareDurableObjectContext, repairDurableObjectAlarm,
} from "../../durable-objects/alarm-shim.js";
import type { AlarmIndexCapability } from "../../durable-objects/protocol.js";
import { tenantConstructor, trackExecutionContext, wrapInstance } from "./runtime.js";
import type { Environment, EnvironmentWrapper } from "./runtime.js";

function alarmIndex(value: unknown): value is AlarmIndexCapability {
  return value !== null && typeof value === "object"
    && "upsert" in value && typeof value.upsert === "function"
    && "delete" in value && typeof value.delete === "function"
    && "clear" in value && typeof value.clear === "function";
}

/** Keep alarm state outside tenant objects and prepare storage before construction. */
export function wrapDurableObject(target: unknown, wrapEnv: EnvironmentWrapper, name: string) {
  const Base = tenantConstructor(target);
  const states = new WeakMap<object, ReturnType<typeof prepareDurableObjectContext>>();
  const stateFor = (instance: object) => {
    const state = states.get(instance);
    if (!state) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
    return state;
  };
  const Wrapped = class extends Base {
    constructor(ctx: DurableObjectState, env: Environment) {
      const wrapped = wrapEnv(env);
      const index = env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX;
      if (!alarmIndex(index)) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
      const prepared = prepareDurableObjectContext(ctx, index);
      const tracked = trackExecutionContext(prepared.context);
      super(prepared.context, wrapped);
      if (Reflect.get(this, "ctx", this) === prepared.context
          && !Reflect.set(this, "ctx", tracked.context, this)) {
        throw new Error("invalid durable object context");
      }
      states.set(this, prepared);
      activateDurableObjectAlarm(prepared);
      return wrapInstance(this, wrapped, tracked);
    }
    async __openComputeAlarm(payload: unknown) {
      return dispatchDurableObjectAlarm(this, Reflect.get(Base.prototype, "alarm"), stateFor(this), payload);
    }
    async __openComputeAlarmRepair() {
      return repairDurableObjectAlarm(stateFor(this));
    }
  };
  Object.defineProperty(Wrapped, "name", { value: name });
  return Wrapped;
}
