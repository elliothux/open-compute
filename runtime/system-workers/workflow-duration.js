// Capability V2 duration grammar; decimal long multiplication mirrors Rust.
export const WORKFLOW_MAX_SAFE_INTEGER = 9007199254740991;
export const WORKFLOW_MAX_DURATION_MS = 365 * 24 * 60 * 60 * 1000;
const encoder = new TextEncoder();
const units = new Map([
  ...["ms", "millisecond", "milliseconds"].map(unit => [unit, 1]),
  ...["s", "second", "seconds"].map(unit => [unit, 1000]),
  ...["m", "minute", "minutes"].map(unit => [unit, 60000]),
  ...["h", "hour", "hours"].map(unit => [unit, 3600000]),
  ...["d", "day", "days"].map(unit => [unit, 86400000]),
  ...["w", "week", "weeks"].map(unit => [unit, 604800000]),
]);
function invalid() { throw new Error("WORKFLOW_DURATION_INVALID"); }

export function durationMs(value, maximum = WORKFLOW_MAX_DURATION_MS) {
  let result;
  if (typeof value === "number") {
    if (!Number.isFinite(value) || value < 0) invalid();
    result = Math.ceil(value);
  } else if (typeof value === "string") {
    if (encoder.encode(value).byteLength > 4096) invalid();
    const match = /^([0-9]+(?:\.[0-9]*)?|\.[0-9]+)\s+([a-z]+)$/i.exec(value.trim());
    if (!match) invalid();
    const multiplier = units.get(match[2].toLowerCase());
    if (multiplier === undefined) invalid();
    const [whole, fraction = ""] = match[1].split(".");
    let integer = 0;
    for (const digit of whole) {
      integer = integer * 10 + (digit.charCodeAt(0) - 48);
      if (!Number.isSafeInteger(integer)) invalid();
    }
    let carry = 0;
    let remainder = false;
    for (let index = fraction.length - 1; index >= 0; index--) {
      const product = (fraction.charCodeAt(index) - 48) * multiplier + carry;
      remainder ||= product % 10 !== 0;
      carry = Math.floor(product / 10);
    }
    result = integer * multiplier + carry + Number(remainder);
  } else invalid();
  if (!Number.isSafeInteger(result) || result > Math.min(maximum, WORKFLOW_MAX_SAFE_INTEGER)) invalid();
  return result === 0 ? 0 : result;
}

export function timestampMs(value) {
  const timestamp = value instanceof Date ? value.getTime() : value;
  if (!Number.isSafeInteger(timestamp)) invalid();
  return timestamp;
}
