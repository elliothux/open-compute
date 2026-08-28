import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { workflowJson } from "../system-workers/workflow-json.js";
import { workflowJson as workflowJsonV2, workflowSerializationCode } from "../system-workers/workflow-json-v2.js";

test("shared Rust and JavaScript wire fixtures", () => {
  const fixtures = JSON.parse(readFileSync(new URL("./fixtures/workflow-json.json",import.meta.url),"utf8"));
  for (const {input,expected} of fixtures) {
    assert.equal(workflowJson(JSON.parse(input)),expected,input);
    assert.equal(workflowJsonV2(JSON.parse(input)),expected,input);
  }
});

test("canonical JSON subset and UTF-8 ordering", () => {
  assert.equal(workflowJson({ z: 1, a: [undefined, NaN, Infinity], no: undefined }), '{"a":[null,null,null],"z":1}');
  assert.equal(workflowJson(undefined), "null");
  assert.equal(workflowJson(-0), "0");
  assert.equal(workflowJson({ "𐀀": 2, "\ue000": 1 }), '{"":1,"𐀀":2}');
  assert.equal(workflowJson(Object.assign(Object.create(null), { a: 1 })), '{"a":1}');
  assert.equal(workflowJson({ toJSON() { throw new Error("must not execute"); }, a: 1 }), '{"a":1}');
});

test("size, depth, cycle, surrogate and non-JSON types are bounded", () => {
  assert.equal(workflowJson("a".repeat(1024 * 1024 - 2)).length, 1024 * 1024);
  assert.throws(() => workflowJson("a".repeat(1024 * 1024 - 1)), /WORKFLOW_RESULT_TOO_LARGE/);
  const cycle = {}; cycle.self = cycle;
  for (const value of [cycle, 1n, new Date(), new Map(), new Set(), new Uint8Array(1),
    new ArrayBuffer(1), new ReadableStream(), "\ud800", { "\udfff": 1 }]) {
    assert.throws(() => workflowJson(value), /WORKFLOW_SERIALIZATION_UNSUPPORTED/);
  }
  let value = null;
  for (let i = 0; i < 127; i++) value = [value];
  assert.doesNotThrow(() => workflowJson(value));
  assert.throws(() => workflowJson([value]), /WORKFLOW_SERIALIZATION_UNSUPPORTED/);
});

test("V2 distinguishes generated size failures without reading hostile exception fields", () => {
  for (const [value, expected] of [
    ["x".repeat(1024*1024), "WORKFLOW_RESULT_TOO_LARGE"],
    [new Date(), "WORKFLOW_SERIALIZATION_UNSUPPORTED"],
    [{get value() {throw new Error("WORKFLOW_RESULT_TOO_LARGE");}}, "WORKFLOW_SERIALIZATION_UNSUPPORTED"],
    [{get value() {throw new Proxy({}, {get() {throw new Error("must not inspect");}});}}, "WORKFLOW_SERIALIZATION_UNSUPPORTED"],
  ]) {
    assert.throws(() => workflowJsonV2(value), error => workflowSerializationCode(error) === expected);
  }
});
