import assert from "node:assert/strict";
import test from "node:test";
import { listOperationIds, operatorOperations } from "../dist/registry.js";

test("operator operation registry lists every registered HTTP operation once", () => {
  const ids = listOperationIds();
  assert.ok(ids.length >= 40);
  assert.equal(new Set(ids).size, ids.length);
  assert.deepEqual(ids, [...ids].sort());
});

test("registry entries bind method, path template, and success schema", () => {
  assert.equal(operatorOperations.system.meta.method, "GET");
  assert.equal(operatorOperations.system.meta.path({}), "meta");
  assert.equal(operatorOperations.workers.promote.idempotent, true);
  assert.equal(
    operatorOperations.kv.getValue.path({
      accountId: "a",
      namespaceId: "ns",
      key: "hello world",
    }),
    "accounts/a/kv/namespaces/ns/values/hello%20world",
  );
  assert.equal(operatorOperations.r2.getObject.method, "GET");
  assert.equal(operatorOperations.durableObjects.listObjects.id, "durableObjects.listObjects");
});
