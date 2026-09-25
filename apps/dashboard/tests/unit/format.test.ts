import { describe, expect, test } from "bun:test";
import { formatBytes, formatDate, formatDateTime } from "../../src/lib/format";

describe("formatBytes", () => {
  test("formats byte values consistently across dashboard products", () => {
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1024)).toBe("1.0 kB");
    expect(formatBytes(1024 ** 3)).toBe("1.0 GB");
  });
});

describe("date formatting", () => {
  test("handles missing and invalid values", () => {
    expect(formatDate(undefined)).toBe("—");
    expect(formatDateTime("not-a-date")).toBe("—");
    expect(formatDate("2026-09-25T00:00:00Z")).not.toBe("—");
  });
});
