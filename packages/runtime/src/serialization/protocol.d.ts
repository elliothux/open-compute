/** Current durable-value profiles. Admitted value sets match; limits and error codes do not. */
export type DurableValueProfile = "queue-v8" | "workflow";

/** Encoder/decoder bounds for one profile. */
export interface DurableValueLimits {
  readonly maxBytes: number;
  readonly maxNodes: number;
  readonly maxDepth: number;
}

/** Stable typed-array classes in the pinned `Rpc.Serializable` surface. */
export type DurableTypedArray =
  | Int8Array
  | Uint8Array
  | Uint8ClampedArray
  | Int16Array
  | Uint16Array
  | Int32Array
  | Uint32Array
  | Float32Array
  | Float64Array
  | BigInt64Array
  | BigUint64Array;

/**
 * Durable structured-clone subset for Queue `v8` bodies and Workflow payloads.
 * Capabilities, streams, HTTP messages, and other host objects are not admitted.
 */
export type DurableValue =
  | null
  | undefined
  | boolean
  | number
  | bigint
  | string
  | Date
  | RegExp
  | Error
  | DOMException
  | ArrayBuffer
  | DataView
  | DurableTypedArray
  | DurableValue[]
  | Map<DurableValue, DurableValue>
  | Set<DurableValue>
  | { [key: string]: DurableValue };
