import { bindingError, doPolicy } from "../loader/shared.js";
import type { DoPolicy, DoPolicyEnv } from "./protocol.js";

let activeDispatches = 0;

/** Reject overload before starting tenant work and retain admission until it settles. */
export function admitted<T>(
  env: DoPolicyEnv,
  operation: (policy: DoPolicy) => Promise<T>,
): Promise<T> {
  const policy = doPolicy(env);
  if (activeDispatches >= policy.maxInFlightDispatches)
    throw bindingError("DO_STORAGE_LIMIT");
  activeDispatches += 1;
  const timeout = new AbortController();
  const pending = Promise.resolve()
    .then(() => operation(policy))
    .finally(() => {
      activeDispatches -= 1;
    });
  return Promise.race([
    pending,
    scheduler
      .wait(policy.dispatchTimeoutMs, { signal: timeout.signal })
      .then(() => {
        throw bindingError("DO_DISPATCH_TIMEOUT");
      }),
  ]).finally(() => timeout.abort());
}
