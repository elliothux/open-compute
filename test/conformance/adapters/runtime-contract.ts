import { readFileSync } from "node:fs";

const lock = JSON.parse(
  readFileSync(
    new URL("../../../packages/runtime/workerd.lock.json", import.meta.url),
    "utf8",
  ),
) as {
  effectiveCompatibilityDate: string;
  requiredCompatibilityFlags: string[];
  workersSdk: { wranglerVersion: string };
};

export const COMPATIBILITY_DATE = lock.effectiveCompatibilityDate;
export const COMPATIBILITY_FLAGS = lock.requiredCompatibilityFlags;
/** Wrangler version coordinated with the formal workerd/workers-types baseline. */
export const WRANGLER_VERSION = lock.workersSdk.wranglerVersion;
export const MAX_OUTPUT = 1024 * 1024;
