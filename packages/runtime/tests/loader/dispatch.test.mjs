import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const { assertEnvelope } = await importRuntime("loader/envelope.ts");

const instanceId = "019c0000000070008000000000000001";
const workerId = "019c0000-0000-7000-8000-000000000002";
const versionId = "019c0000-0000-7000-8000-000000000003";

function request(key, headerId = instanceId) {
  return new Request("https://worker.invalid/", {
    headers: {
      "x-open-compute-loader-key": key,
      "x-open-compute-instance-id": headerId,
      "x-open-compute-worker-code-sha256": "a".repeat(64),
      "x-open-compute-route-generation": "1",
    },
  });
}

test("dispatch accepts only the current scoped loader identity", () => {
  const key = `${instanceId}/${workerId}/${versionId}`;
  assert.equal(assertEnvelope(request(key), false).loaderKey, key);
  for (const [candidate, headerId] of [
    [key, "019c0000000070008000000000000004"],
    [`${workerId}/${workerId}/${versionId}`, instanceId],
    [`${instanceId}/${workerId}/${versionId}/extra`, instanceId],
  ]) {
    assert.throws(() => assertEnvelope(request(candidate, headerId), false));
  }
});
