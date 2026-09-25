import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const shared = moduleUrl(
  'export const currentStartupGeneration = () => "generation";',
);
const { collectObservabilityTail } = await import(
  moduleUrl(
    await compileRuntime("observability/collector.ts", {
      "../loader/shared.js": shared,
    }),
  )
);

const identity = {
  schemaVersion: 1,
  instanceId: "019c0000000070008000000000000001",
  workerId: "019c0000-0000-7000-8000-000000000002",
  scriptName: "worker",
  versionId: "019c0000-0000-7000-8000-000000000003",
  routeGeneration: 1,
  observabilityGeneration: 1,
};
const event = {
  truncated: false,
  logs: [],
  exceptions: [],
  outcome: "ok",
  eventTimestamp: 1,
  event: {},
  executionModel: "stateless",
  cpuTime: 0,
  wallTime: 0,
};

test("collector admits canonical instance identity and rejects old UUID shape", async () => {
  const bodies = [];
  const env = {
    OBSERVABILITY_BACKEND_TOKEN: "private-token",
    OBSERVABILITY_BACKEND: {
      async fetch(_url, init) {
        bodies.push(JSON.parse(new TextDecoder().decode(init.body)));
        return Response.json({ ok: true });
      },
    },
  };
  await collectObservabilityTail([event], env, identity);
  assert.equal(bodies.length, 1);
  assert.equal(bodies[0].identity.instanceId, identity.instanceId);
  assert.equal(Object.hasOwn(bodies[0].identity, "accountId"), false);

  await collectObservabilityTail([event], env, {
    ...identity,
    instanceId: "019c0000-0000-7000-8000-000000000001",
  });
  assert.equal(bodies.length, 1);
});
