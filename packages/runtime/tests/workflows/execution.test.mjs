import assert from "node:assert/strict";
import test from "node:test";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const workerModule = moduleUrl(`export class RpcTarget {}
  export class WorkflowEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }`);
const { WorkflowEntrypoint } = await import(workerModule);
const { WorkflowRunController, finishWorkflowRun, closeWorkflowRun } = await importRuntime("workflows/controller.ts", {
  "cloudflare:workers": workerModule,
  "../loader/host.js": moduleUrl('export const currentStartupGeneration = () => "current-generation";'),
});
const { runWorkflow } = await importRuntime("workflows/runner.ts", {
  "cloudflare:workers": workerModule,
  "cloudflare:workflows": moduleUrl("export class NonRetryableError extends Error {}"),
  "./json.js": moduleUrl(await compileRuntime("workflows/json.ts")),
});
const identity = { instanceId: "private-instance", instanceGeneration: 1, runToken: "private-run-token" };
const event = { externalInstanceId: "order", definitionName: "flow", createdAtMs: 123, payloadJson: '{"value":1}' };
const declaration = { ordinal: 0, kind: "do", name: "compute", nameCount: 1, config: {},
  dependencies: [], batchFirstOrdinal: 0, batchSize: 1 };
const config = { timeout: 1000, retries: { limit: 0, delay: 0, backoff: "constant" } };

function controller(t, fetch) {
  const value = new WorkflowRunController({ BINDING_BACKEND: { fetch }, BINDING_BACKEND_TOKEN: "private-backend-token" }, identity, 10_000);
  t.after(() => value[closeWorkflowRun]());
  return value;
}

test("compiled Workflow controller keeps grants private and fences duplicate commits", async t => {
  const calls = [];
  const value = controller(t, async (url, request) => {
    const body = JSON.parse(request.body);
    calls.push({ url, body, headers: request.headers });
    return Response.json(url.endsWith("/claim-batch")
      ? { steps: [{ state: "run", stepToken: "a".repeat(64), attempt: 1, remainingMs: 10_000, config }] }
      : { state: "complete", outputJson: "42" });
  });
  const claim = await value.claimBatch({ steps: [declaration] });
  assert.deepEqual(claim, { steps: [{ ordinal: 0, state: "run", attempt: 1, config }] });
  assert.deepEqual(Object.keys(value), []);
  assert.ok(!JSON.stringify(claim).includes("Token"));
  const result = value.result(0);
  assert.deepEqual(await value.success({ ordinal: 0, outputJson: "42", stepToken: "forged", runToken: "forged" }),
    { state: "complete", outputJson: "42" });
  assert.equal(calls[1].body.stepToken, "a".repeat(64));
  assert.equal(calls[1].body.runToken, identity.runToken);
  assert.equal(calls[1].headers["x-open-compute-startup-generation"], "current-generation");
  assert.deepEqual(await result, { state: "complete", outputJson: "42" });
  assert.deepEqual(await value.success({ ordinal: 0, outputJson: "43" }), { errorCode: "WORKFLOW_STEP_STALE" });
  assert.equal(calls.length, 2);
  assert.deepEqual(await value.drain(), { ok: true });
  const final = { outcome: "complete", outputJson: "42", finalOrdinal: 1 };
  assert.deepEqual(await value[finishWorkflowRun](final), { result: final, drainIncomplete: false });
});

test("malformed Workflow verdicts permanently close admission for that activation", async t => {
  for (const reply of [null, { state: "complete", outputJson: 42 }, { state: "failed", code: 42 }, { state: "unknown" }]) {
    const value = controller(t, async () => Response.json(reply));
    await assert.rejects(value.result(0), /WORKFLOW_RUNTIME_UNAVAILABLE/);
    await assert.rejects(value[finishWorkflowRun]({ outcome: "complete", outputJson: '"caught"', finalOrdinal: 0 }), /WORKFLOW_RUNTIME_UNAVAILABLE/);
  }
});

test("compiled Workflow runner replays parallel steps without executing their callbacks", async () => {
  const declarations = [];
  class Flow extends WorkflowEntrypoint {
    async run(value, step) {
      assert.equal(value.timestamp.getTime(), 123);
      assert.deepEqual(value.payload, { value: 1 });
      const never = () => { throw new Error("replayed callback executed"); };
      return Promise.all([step.do("first", never), step.do("second", never)]);
    }
  }
  const result = await runWorkflow(Flow, {}, {}, event, {
    async claimBatch(body) {
      declarations.push(...body.steps);
      return { steps: body.steps.map(step => ({ ordinal: step.ordinal, state: "complete" })) };
    },
    async result(ordinal) { return { state: "complete", outputJson: String(ordinal + 1) }; },
    async drain() { return { ok: true }; },
  });
  assert.deepEqual(result, { outcome: "complete", outputJson: "[1,2]", finalOrdinal: 2 });
  assert.deepEqual(declarations.map(step => [step.ordinal, step.batchFirstOrdinal, step.batchSize]), [[0, 0, 2], [1, 0, 2]]);
});

test("catching a malformed Workflow event cannot turn a protocol failure into success", async () => {
  class Flow extends WorkflowEntrypoint {
    async run(_event, step) {
      try { await step.waitForEvent("approval", { type: "approved" }); }
      catch { return "caught"; }
    }
  }
  for (const outputJson of ['{"type":42}', "invalid JSON"]) {
    await assert.rejects(runWorkflow(Flow, {}, {}, event, {
      async registerWait() { return { state: "complete", outputJson }; },
    }), /WORKFLOW_RUNTIME_UNAVAILABLE/);
  }
});
