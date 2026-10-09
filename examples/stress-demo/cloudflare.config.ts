import { bindings, defineConfig, exports } from "cf/config";

// This config is also imported for Worker type inference, without Node global types.
const hostProcess: unknown = Reflect.get(globalThis, "process");
if (typeof hostProcess !== "object" || hostProcess === null)
  throw new Error("cf configuration requires a host process");
const rawEnvironment: unknown = Reflect.get(hostProcess, "env");
if (typeof rawEnvironment !== "object" || rawEnvironment === null)
  throw new Error("cf configuration requires environment values");

const env = rawEnvironment;

function required(name: string): string {
  const value: unknown = Reflect.get(env, name);
  if (typeof value !== "string" || !value)
    throw new Error(`set ${name} using test/stress/deploy-stress-demo.sh`);
  return value;
}

export default defineConfig({
  worker: {
    name: "stress-demo",
    entrypoint: "./src/index.ts",
    compatibilityDate: "2026-09-08",
    compatibilityFlags: ["nodejs_compat"],
    workersDev: false,
    exports: {
      default: exports.worker(),
      InternalApi: exports.worker(),
      Inventory: exports.durableObject({ storage: "sqlite" }),
      CheckoutFlow: exports.workflow({ name: "stress-demo-checkout-flow" }),
    },
    env: {
      REVISION: bindings.text("p0-stress"),
      TOKEN: bindings.secret(),
      OUTBOUND_URL: bindings.text(required("STRESS_OUTBOUND_URL")),
      KV: bindings.kv({ id: required("STRESS_KV_NAMESPACE_ID") }),
      DB: bindings.d1({
        id: required("STRESS_D1_DATABASE_ID"),
        name: "stress-demo-db",
      }),
      BUCKET: bindings.r2({ name: "stress-demo-bucket" }),
      EVENTS: bindings.queue({ name: "stress-demo-events" }),
      INVENTORY: bindings.durableObject({
        worker: "stress-demo",
        exportName: "Inventory",
      }),
      FLOW: bindings.workflow({
        name: "stress-demo-checkout-flow",
        worker: "stress-demo",
        exportName: "CheckoutFlow",
      }),
      ...(Reflect.get(env, "STRESS_INCLUDE_SERVICE") === "true"
        ? {
            SERVICE: bindings.worker({
              worker: "stress-demo",
              exportName: "InternalApi",
            }),
          }
        : {}),
    },
  },
});
