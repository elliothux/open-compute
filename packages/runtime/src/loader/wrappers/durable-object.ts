import {
  activateDurableObjectAlarm, dispatchDurableObjectAlarm,
  prepareDurableObjectContext, repairDurableObjectAlarm,
} from "../../durable-objects/alarm-shim.js";
import { runWithOutputGate } from "../../durable-objects/output-gate.js";
import { prepareTenantFacets } from "../../durable-objects/facets.js";
import type {
  AlarmIndexCapability, FacetManagerCapability, TenantDoAuthority,
} from "../../durable-objects/protocol.js";
import {
  tenantConstructor, trackExecutionContext, trustedContextExports, wrapInstance,
} from "./runtime.js";
import type { Environment, EnvironmentWrapper } from "./runtime.js";

function alarmIndex(value: unknown): value is AlarmIndexCapability {
  return value !== null && typeof value === "object"
    && "upsert" in value && typeof value.upsert === "function"
    && "delete" in value && typeof value.delete === "function"
    && "clear" in value && typeof value.clear === "function";
}

function facetManager(value: unknown): value is FacetManagerCapability {
  return value !== null && typeof value === "object"
    && typeof Reflect.get(value, "__openComputeFacetCall") === "function"
    && typeof Reflect.get(value, "__openComputeFacetClone") === "function";
}

function authority(value: unknown): value is TenantDoAuthority {
  return value !== null && typeof value === "object"
    && typeof Reflect.get(value, "accountId") === "string"
    && typeof Reflect.get(value, "workerId") === "string"
    && typeof Reflect.get(value, "versionId") === "string"
    && typeof Reflect.get(value, "workerCodeSha256") === "string"
    && typeof Reflect.get(value, "className") === "string";
}

function tenantContext(
  source: DurableObjectState,
  props: unknown,
  facets: DurableObjectFacets,
): DurableObjectState {
  if (!Reflect.defineProperty(source, "props", {
    value: props,
    configurable: false,
    enumerable: true,
    writable: false,
  }) || !Reflect.defineProperty(source, "facets", {
    value: facets,
    configurable: false,
    enumerable: true,
    writable: false,
  })) {
    throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return source;
}

/** Keep alarm state outside tenant objects and prepare storage before construction. */
export function wrapDurableObject(target: unknown, wrapEnv: EnvironmentWrapper, name: string) {
  const Base = tenantConstructor(target);
  const states = new WeakMap<object, ReturnType<typeof prepareDurableObjectContext> | undefined>();
  const stateFor = (instance: object) => {
    const state = states.get(instance);
    if (!state) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
    return state;
  };
  const Wrapped = class extends Base {
    constructor(ctx: DurableObjectState, env: Environment) {
      const trustedExports = trustedContextExports(ctx);
      const wrapped = wrapEnv(env);
      const index = env.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX;
      const manager = env.__OPEN_COMPUTE_PRIVATE_FACET_MANAGER;
      const resolvedAuthority = env.__OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY;
      const logicalPath = env.__OPEN_COMPUTE_PRIVATE_FACET_PATH;
      const tenantProps = env.__OPEN_COMPUTE_PRIVATE_FACET_PROPS;
      if (!alarmIndex(index)) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
      if (!facetManager(manager) || !authority(resolvedAuthority)) {
        throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
      }
      const logical = prepareTenantFacets(
        ctx,
        manager,
        resolvedAuthority,
        logicalPath,
        tenantProps,
      );
      const prepared = logical.logicalPath.length === 0
        ? prepareDurableObjectContext(ctx, index)
        : undefined;
      const context = tenantContext(prepared?.context ?? ctx, logical.tenantProps, logical.facets);
      const tracked = trackExecutionContext(
        context,
        undefined,
        prepared === undefined ? undefined : fn => runWithOutputGate(prepared.gate, fn),
        true,
        trustedExports,
      );
      const safeExports: unknown = Reflect.get(tracked.context, "exports", tracked.context);
      if (safeExports === null || typeof safeExports !== "object"
          || !Reflect.defineProperty(context, "exports", {
            value: safeExports,
            configurable: false,
            enumerable: true,
            writable: false,
          })) {
        throw new Error("DO_RUNTIME_EXCEPTION");
      }
      super(context, wrapped);
      if (!Reflect.defineProperty(this, "ctx", {
        value: tracked.context,
        configurable: false,
        enumerable: true,
        writable: false,
      })) {
        throw new Error("DO_RUNTIME_EXCEPTION");
      }
      states.set(this, prepared);
      if (prepared !== undefined) activateDurableObjectAlarm(prepared, wrapped);
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
