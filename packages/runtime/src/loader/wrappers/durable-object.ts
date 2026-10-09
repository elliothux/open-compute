import type { CacheRuntimeFactory } from "../../cache/facade.js";
import {
  activateDurableObjectAlarm,
  dispatchDurableObjectAlarm,
  prepareDurableObjectContext,
  repairDurableObjectAlarm,
} from "../../durable-objects/alarm-shim.js";
import { prepareTenantFacets } from "../../durable-objects/facets.js";
import { createFetchAdmission } from "../../durable-objects/fetch-admission.js";
import { assertRpcMember } from "../../durable-objects/host-protocol.js";
import { runWithOutputGate } from "../../durable-objects/output-gate.js";
import type {
  AlarmIndexCapability,
  FacetManagerCapability,
  TenantDoAuthority,
} from "../../durable-objects/protocol.js";
import { privateWeakMap } from "../../private-weak-map.js";
import { completeServiceScope } from "../../services/facade.js";
import { serviceRpcMember } from "../../services/rpc-member.js";
import { currentServiceFrame } from "../../services/scope.js";
import type { NativeHostFacets } from "../protocol.js";
import {
  tenantConstructor,
  trackExecutionContext,
  trustedContextExports,
  wrapInstance,
  type Environment,
} from "./runtime.js";

const nativeGet = Reflect.get;
const nativeDefine = Reflect.defineProperty;
const nativeApply = Reflect.apply;

function nativeHostFacets(value: unknown): value is NativeHostFacets {
  return (
    value !== null &&
    typeof value === "object" &&
    typeof nativeGet(value, "create") === "function" &&
    typeof nativeGet(value, "revoke") === "function"
  );
}

function alarmIndex(value: unknown): value is AlarmIndexCapability {
  return (
    value !== null &&
    typeof value === "object" &&
    "upsert" in value &&
    typeof value.upsert === "function" &&
    "delete" in value &&
    typeof value.delete === "function" &&
    "clear" in value &&
    typeof value.clear === "function"
  );
}

function facetManager(value: unknown): value is FacetManagerCapability {
  return (
    value !== null &&
    typeof value === "object" &&
    typeof nativeGet(value, "__openComputeFacetCall") === "function" &&
    typeof nativeGet(value, "__openComputeFacetClone") === "function"
  );
}

function authority(value: unknown): value is TenantDoAuthority {
  return (
    value !== null &&
    typeof value === "object" &&
    typeof nativeGet(value, "instanceId") === "string" &&
    typeof nativeGet(value, "workerId") === "string" &&
    typeof nativeGet(value, "versionId") === "string" &&
    typeof nativeGet(value, "workerCodeSha256") === "string" &&
    typeof nativeGet(value, "className") === "string"
  );
}

function tenantContext(
  source: DurableObjectState,
  props: unknown,
  facets: DurableObjectFacets,
): DurableObjectState {
  if (
    !nativeDefine(source, "props", {
      value: props,
      configurable: false,
      enumerable: true,
      writable: false,
    }) ||
    !nativeDefine(source, "facets", {
      value: facets,
      configurable: false,
      enumerable: true,
      writable: false,
    })
  ) {
    throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
  }
  return source;
}

/** Keep alarm state outside tenant objects and prepare storage before construction. */
export function wrapDurableObject(
  target: unknown,
  privateEnvironment: Environment,
  name: string,
  cache?: CacheRuntimeFactory,
) {
  const Base = tenantConstructor(target);
  const states = privateWeakMap<
    object,
    ReturnType<typeof prepareDurableObjectContext> | undefined
  >();
  const instances = privateWeakMap<
    object,
    { instance: object; admission: ReturnType<typeof createFetchAdmission> }
  >();
  const stateFor = (instance: object) => {
    const state = states.get(instance);
    if (!state) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
    return state;
  };
  const Wrapped = class extends Base {
    constructor(ctx: DurableObjectState, env: Environment) {
      cache?.bind();
      const trustedExports = trustedContextExports(ctx);
      const wrapped = env;
      const index = privateEnvironment.__OPEN_COMPUTE_PRIVATE_ALARM_INDEX;
      const manager = privateEnvironment.__OPEN_COMPUTE_PRIVATE_FACET_MANAGER;
      const resolvedAuthority =
        privateEnvironment.__OPEN_COMPUTE_PRIVATE_FACET_AUTHORITY;
      const logicalPath = privateEnvironment.__OPEN_COMPUTE_PRIVATE_FACET_PATH;
      const tenantProps = privateEnvironment.__OPEN_COMPUTE_PRIVATE_FACET_PROPS;
      const nativeFacets =
        privateEnvironment.__OPEN_COMPUTE_PRIVATE_NATIVE_FACETS;
      if (!alarmIndex(index)) throw new Error("DO_ALARM_INDEX_UNAVAILABLE");
      if (
        !facetManager(manager) ||
        !authority(resolvedAuthority) ||
        !nativeHostFacets(nativeFacets)
      ) {
        throw new Error("DO_INTERNAL_PROTOCOL_ERROR");
      }
      const logical = prepareTenantFacets(
        ctx,
        manager,
        resolvedAuthority,
        logicalPath,
        tenantProps,
        nativeFacets,
      );
      const prepared =
        logical.logicalPath.length === 0
          ? prepareDurableObjectContext(ctx, index)
          : undefined;
      const context = tenantContext(
        prepared?.context ?? ctx,
        logical.tenantProps,
        logical.facets,
      );
      const tracked = trackExecutionContext(
        context,
        undefined,
        prepared === undefined
          ? undefined
          : (fn) => runWithOutputGate(prepared.gate, fn),
        trustedExports,
      );
      const blockConcurrencyWhile = context.blockConcurrencyWhile;
      if (
        !nativeDefine(context, "blockConcurrencyWhile", {
          configurable: true,
          writable: true,
          value(callback: unknown) {
            if (typeof callback !== "function")
              return nativeApply(blockConcurrencyWhile, context, [callback]);
            const frame = currentServiceFrame();
            return nativeApply(blockConcurrencyWhile, context, [
              async () => {
                try {
                  return await nativeApply(callback, undefined, []);
                } catch (error) {
                  // Native input-gate failure shuts down the actor without rejecting its returned promise.
                  await completeServiceScope(wrapped, frame).catch(
                    () => undefined,
                  );
                  throw error;
                }
              },
            ]);
          },
        })
      )
        throw new Error("DO_RUNTIME_EXCEPTION");
      const safeExports: unknown = nativeGet(
        tracked.context,
        "exports",
        tracked.context,
      );
      if (
        safeExports === null ||
        typeof safeExports !== "object" ||
        !nativeDefine(context, "exports", {
          value: safeExports,
          configurable: false,
          enumerable: true,
          writable: false,
        })
      ) {
        throw new Error("DO_RUNTIME_EXCEPTION");
      }
      super(context, wrapped);
      if (
        !nativeDefine(this, "ctx", {
          value: tracked.context,
          configurable: false,
          enumerable: true,
          writable: false,
        })
      ) {
        throw new Error("DO_RUNTIME_EXCEPTION");
      }
      states.set(this, prepared);
      if (prepared !== undefined) activateDurableObjectAlarm(prepared, wrapped);
      const admission = createFetchAdmission(ctx);
      const instance = wrapInstance(
        this,
        wrapped,
        tracked,
        undefined,
        hostMethods,
        admission.start,
      );
      instances.set(this, { instance, admission });
      return instance;
    }
    async __openComputeInvokeRpc(
      kind: "call" | "get",
      method: string,
      args: unknown[],
      started: () => Promise<void>,
    ): Promise<unknown> {
      assertRpcMember(method);
      const instance = instances.get(this)?.instance;
      if (!instance || !Array.isArray(args) || typeof started !== "function")
        throw new Error("DO_RPC_UNSUPPORTED");
      let value: unknown;
      if (kind === "call") {
        const target = serviceRpcMember(instance, method, "call");
        value = nativeApply(target, instance, args);
      } else if (kind === "get") {
        value = serviceRpcMember(instance, method, "get");
      } else throw new Error("DO_RPC_UNSUPPORTED");
      await started();
      return value;
    }
    __openComputePrepareFetch(
      token: string,
      started: Rpc.Stub<() => Promise<void>>,
      timeoutMs: number,
    ): void {
      const value = instances.get(this);
      if (!value) throw new Error("DO_RUNTIME_EXCEPTION");
      value.admission.prepare(token, started, timeoutMs);
    }
    __openComputeCancelFetch(token: string): void {
      instances.get(this)?.admission.cancel(token);
    }
    async __openComputeAlarm(payload: unknown) {
      return dispatchDurableObjectAlarm(
        this,
        nativeGet(this, "alarm", this),
        stateFor(this),
        payload,
      );
    }
    async __openComputeAlarmRepair() {
      return repairDurableObjectAlarm(stateFor(this));
    }
  };
  const hostMethods = {
    __openComputeInvokeRpc: Wrapped.prototype.__openComputeInvokeRpc,
    __openComputePrepareFetch: Wrapped.prototype.__openComputePrepareFetch,
    __openComputeCancelFetch: Wrapped.prototype.__openComputeCancelFetch,
    __openComputeAlarm: Wrapped.prototype.__openComputeAlarm,
    __openComputeAlarmRepair: Wrapped.prototype.__openComputeAlarmRepair,
  };
  Object.defineProperty(Wrapped, "name", { value: name });
  return Wrapped;
}
