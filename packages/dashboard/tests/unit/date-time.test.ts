import { describe, expect, test } from "bun:test";
import {
  formatRelative,
  formatTimestamp,
  normalizeRfc3339,
} from "../../src/lib/date-time";

describe("date-time boundary", () => {
  test("normalizes valid wire values and rejects malformed input", () => {
    expect(normalizeRfc3339(0)).toBe("1970-01-01T00:00:00.000Z");
    expect(normalizeRfc3339("2026-09-08T12:30:00Z")).toBe(
      "2026-09-08T12:30:00.000Z",
    );
    expect(normalizeRfc3339("not-a-date")).toBeNull();
  });

  test("relative output uses an injected clock", () => {
    expect(formatRelative(0, { now: () => 60_000 })).toBe("1 minute ago");
    expect(formatTimestamp(Number.NaN)).toBe("—");
  });
});
