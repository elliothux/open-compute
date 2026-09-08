import type { ServiceFrame } from "../../services/scope.js";

/** Explicit tenant variables and product capabilities, never host service bindings. */
export type Environment = Record<string, unknown>;
export type EnvironmentWrapper = (env: Environment) => Environment;
export interface BindingFactory {
  names: readonly string[];
  create: new (raw: unknown, durableObject: boolean, name: string) => object;
}
export interface TenantConstructor {
  new (ctx: unknown, env: Environment): object;
  readonly prototype: object;
}
export interface CompletionReporter extends Disposable {
  beginCapability(retention: string, frame: ServiceFrame): unknown;
  releaseRetention(retention: string): unknown;
  completeOperation(handle: string): unknown;
  retainCapability(handle: string, owner: "caller" | "target"): unknown;
  dup(): CompletionReporter;
}
export interface TrackedContext<Context extends object = object> {
  readonly context: Context;
  readonly tasks: Promise<unknown>[];
  readonly extendLifetime: (promise: Promise<unknown>) => void;
  readonly runScope?: <T>(fn: () => T) => T;
}
export type Callable = (this: unknown, ...args: unknown[]) => unknown;
