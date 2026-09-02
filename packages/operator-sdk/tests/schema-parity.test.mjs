import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import {
  D1DatabasesResponseSchema,
  DoNamespacesResponseSchema,
  ErrorCodeSchema,
  KvNamespacesResponseSchema,
  QueuesResponseSchema,
  R2BucketsResponseSchema,
  WorkflowsResponseSchema,
} from "../dist/index.js";

test("SDK error codes exactly match the canonical Rust wire contract", () => {
  const rust = readFileSync(new URL("../../../crates/core/src/error.rs", import.meta.url), "utf8");
  const start = rust.indexOf("pub const fn as_str(self)");
  const end = rust.indexOf("impl Display for ErrorCode", start);
  assert.notEqual(start, -1, "ErrorCode::as_str was not found");
  assert.notEqual(end, -1, "ErrorCode Display boundary was not found");

  const expected = [...rust.slice(start, end).matchAll(/=> \"([A-Z0-9_]+)\"/g)]
    .map(match => match[1].toLowerCase())
    .sort();
  const actual = [...ErrorCodeSchema.options].sort();

  assert.deepEqual(actual, expected);
});

const kvFixture = {
  namespaces: [{
    resource: {
      id: "550e8400-e29b-41d4-a716-446655440000",
      accountId: "550e8400-e29b-41d4-a716-446655440001",
      kind: "kv_namespace",
      name: "demo",
      state: "ready",
      availability: "healthy",
      specGeneration: 1,
      driverSchemaVersion: 1,
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_100_000,
    },
    schemaVersion: 1,
    quotaBytes: 1_073_741_824,
  }],
};

const d1Fixture = {
  databases: [{
    resource: {
      id: "550e8400-e29b-41d4-a716-446655440010",
      accountId: "550e8400-e29b-41d4-a716-446655440001",
      kind: "d1_database",
      name: "analytics",
      state: "ready",
      availability: "healthy",
      createdAtMs: 1_700_000_000_000,
      updatedAtMs: 1_700_000_100_000,
    },
    schemaVersion: 1,
    quotaBytes: 1_073_741_824,
  }],
};

const r2Fixture = {
  buckets: [{
    resourceId: "550e8400-e29b-41d4-a716-446655440020",
    name: "assets",
    state: "ready",
    availability: "healthy",
    createdAtMs: 1_700_000_000_000,
    updatedAtMs: 1_700_000_100_000,
    maxObjectBytes: 26_214_400,
  }],
};

const doFixture = {
  namespaces: [{
    resourceId: "550e8400-e29b-41d4-a716-446655440030",
    name: "chat",
    state: "ready",
    ownerWorkerId: "550e8400-e29b-41d4-a716-446655440031",
    className: "ChatRoom",
    schemaVersion: 1,
    createdAtMs: 1_700_000_000_000,
  }],
};

const queuesFixture = {
  queues: [{
    id: "550e8400-e29b-41d4-a716-446655440040",
    accountId: "550e8400-e29b-41d4-a716-446655440001",
    name: "jobs",
    state: "ready",
    availability: "healthy",
    lifecycleGeneration: 1,
    configGeneration: 1,
    deliveryDelaySeconds: 0,
    retentionSeconds: 86_400,
    maxMessageBytes: 131_072,
    maxBatchMessages: 100,
    maxBatchBytes: 1_048_576,
    maxBacklogBytes: 1_073_741_824,
    createdAtMs: 1_700_000_000_000,
    updatedAtMs: 1_700_000_100_000,
  }],
  nextCursor: null,
};

const workflowsFixture = {
  workflows: [{
    id: "550e8400-e29b-41d4-a716-446655440050",
    accountId: "550e8400-e29b-41d4-a716-446655440001",
    name: "ingest",
    state: "ready",
    availability: "healthy",
    lifecycleGeneration: 1,
    currentVersionId: null,
    createdAtMs: 1_700_000_000_000,
    updatedAtMs: 1_700_000_100_000,
  }],
  nextCursor: null,
};

test("KV namespaces response matches canonical wire fixture", () => {
  assert.doesNotThrow(() => KvNamespacesResponseSchema.parse(kvFixture));
});

test("D1 databases response matches canonical wire fixture", () => {
  assert.doesNotThrow(() => D1DatabasesResponseSchema.parse(d1Fixture));
});

test("R2 buckets response matches canonical wire fixture", () => {
  assert.doesNotThrow(() => R2BucketsResponseSchema.parse(r2Fixture));
});

test("DO namespaces response matches canonical wire fixture", () => {
  assert.doesNotThrow(() => DoNamespacesResponseSchema.parse(doFixture));
});

test("Queues response matches canonical wire fixture", () => {
  assert.doesNotThrow(() => QueuesResponseSchema.parse(queuesFixture));
});

test("Workflows response matches canonical wire envelope", () => {
  assert.doesNotThrow(() => WorkflowsResponseSchema.parse(workflowsFixture));
});
