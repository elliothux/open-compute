/** Day1 durable-value codec shared by persisted Queue and Workflow values. */
export type { DurableTypedArray, DurableValue, DurableValueLimits, DurableValueProfile } from "./protocol.js";
export {
  DURABLE_VALUE_LIMITS, DURABLE_VALUE_MAGIC, DURABLE_VALUE_PROFILE_ID, DURABLE_VALUE_PROFILES,
  DURABLE_VALUE_SCHEMA, durableValueErrorCode, durableValueLimits,
} from "./format.js";
export { encodeDurableValue } from "./encode.js";
export { decodeDurableValue } from "./decode.js";
