import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { durationMs, timestampMs } from "../system-workers/workflow-duration.js";

test("duration grammar and rounding match shared Rust fixtures", () => {
  const fixtures = JSON.parse(readFileSync(new URL("./fixtures/workflow-duration.json", import.meta.url), "utf8"));
  for (const { input, maximum, expected } of fixtures) {
    if (expected === null) assert.throws(() => durationMs(input, maximum), /WORKFLOW_DURATION_INVALID/, JSON.stringify(input));
    else assert.equal(durationMs(input, maximum), expected, JSON.stringify(input));
  }
});

test("non-finite, non-JSON and excessively long inputs are rejected", () => {
  for (const input of [NaN, Infinity, -Infinity, undefined, 1n, new Date(),
    `${"0".repeat(4096)} ms`, { toString() { throw new Error("must not execute"); } }]) {
    assert.throws(() => durationMs(input), /WORKFLOW_DURATION_INVALID/);
  }
  assert.equal(durationMs(`0.${"0".repeat(4000)}1 weeks`), 1);
});

test("absolute timestamps stay integral and preserve past dates", () => {
  for (const value of [0, -1, 9007199254740991, -9007199254740991]) assert.equal(timestampMs(value), value);
  assert.equal(timestampMs(new Date(-1)), -1);
  for (const value of [NaN, Infinity, new Date(NaN), 0.5, 9007199254740992, "1970-01-01", null]) {
    assert.throws(() => timestampMs(value), /WORKFLOW_DURATION_INVALID/);
  }
});
