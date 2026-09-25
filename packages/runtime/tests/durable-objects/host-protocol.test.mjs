import assert from "node:assert/strict";
import test from "node:test";
import { importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const { authorityFromHeaders } = await importRuntime(
  "durable-objects/host-protocol.ts",
  {
    "../loader/shared.js": moduleUrl(
      "export const bindingError = (code) => new Error(code);",
    ),
  },
);

test("DO host authority accepts only the canonical instance ID", () => {
  const headers = new Headers({
    "x-open-compute-instance-id": "019c0000000070008000000000000001",
    "x-open-compute-worker-id": "019c0000-0000-7000-8000-000000000002",
    "x-open-compute-version-id": "019c0000-0000-7000-8000-000000000003",
    "x-open-compute-worker-code-sha256": "a".repeat(64),
    "x-open-compute-object-id": "b".repeat(64),
    "x-open-compute-namespace-resource-id":
      "019c0000-0000-7000-8000-000000000004",
    "x-open-compute-class-name": "Object",
    "x-open-compute-route-generation": "1",
    "x-open-compute-object-generation": "1",
  });
  assert.equal(
    authorityFromHeaders(headers).instanceId,
    "019c0000000070008000000000000001",
  );
  headers.set(
    "x-open-compute-instance-id",
    "019c0000-0000-7000-8000-000000000001",
  );
  assert.throws(
    () => authorityFromHeaders(headers),
    /DO_INTERNAL_PROTOCOL_ERROR/,
  );
});
