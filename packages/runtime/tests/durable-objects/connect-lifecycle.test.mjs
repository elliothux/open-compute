import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

function source(path) {
  return readFileSync(path, "utf8");
}

function section(text, start, end) {
  const from = text.indexOf(start);
  const to = text.indexOf(end, from + start.length);
  assert.notEqual(from, -1, `missing ${start}`);
  assert.notEqual(to, -1, `missing ${end}`);
  return text.slice(from, to);
}

test("DO connect handoffs use bounded lazy expiry without fixed waitUntil timers", () => {
  const loader = section(
    source("packages/runtime/src/loader/host.ts"),
    "async prepareConnect(",
    "async connect(socket:",
  );
  const router = section(
    source("packages/runtime/src/durable-objects/router.ts"),
    "async function prepareNativeConnect(",
    "async function deleteObject(",
  );
  const host = section(
    source("packages/runtime/src/durable-objects/host.ts"),
    "async __openComputePrepareConnect(",
    "async connect(socket:",
  );

  for (const implementation of [loader, router]) {
    assert.doesNotMatch(implementation, /waitUntil\s*\(/);
    assert.doesNotMatch(implementation, /scheduler\.wait\s*\(/);
    assert.match(implementation, /expiresAt <= now/);
    assert.match(implementation, /\.size >= (?:128|1024)/);
  }
  assert.doesNotMatch(host, /scheduler\.wait\s*\(/);
  assert.match(host, /this\.#purgeExpiredConnects\(now\)/);
  assert.match(host, /\.size >= 128/);
  const hostSource = source("packages/runtime/src/durable-objects/host.ts");
  assert.match(hostSource, /pending\.expiresAt > now/);
  assert.match(hostSource, /waitUntil\(ordered\(/);
});
