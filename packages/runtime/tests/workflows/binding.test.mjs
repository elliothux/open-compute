import test from "node:test";
import assert from "node:assert/strict";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const format = moduleUrl(await compileRuntime("serialization/format.ts"));
const encode = moduleUrl(await compileRuntime("serialization/encode.ts", { "./format.js": format }));
const decode = moduleUrl(await compileRuntime("serialization/decode.ts", { "./format.js": format }));
const durableCodec = moduleUrl(await compileRuntime("serialization/codec.ts", {
  "./format.js": format, "./encode.js": encode, "./decode.js": decode,
}));
const codec = moduleUrl(await compileRuntime("workflows/codec.ts", {
  "../serialization/codec.js": durableCodec,
}));
const { encodeWorkflowBase64 } = await import(codec);
const asyncHooks = moduleUrl(`
  export class AsyncLocalStorage {
    constructor() { this.stack = []; }
    run(store, fn) { this.stack.push(store); try { return fn(); } finally { this.stack.pop(); } }
    getStore() { return this.stack.at(-1); }
  }
`);
const outputGate = moduleUrl(await compileRuntime("durable-objects/output-gate.ts", {
  "node:async_hooks": asyncHooks,
}));
const { WorkflowBinding, triggerWorkflowSchedule } = await import(moduleUrl(await compileRuntime("workflows/facade.ts", {
  "./codec.js": codec,
  "../durable-objects/output-gate.js": outputGate,
})));

test("durable facade resolves once and keeps only a private instance-scoped handle", async () => {
  const calls = [];
  const handle = {
    status: () => ({ status: "queued" }),
    pause: () => calls.push("pause"), resume: () => calls.push("resume"),
    terminate: () => calls.push("terminate"), restart: () => calls.push("restart"),
    delete: () => calls.push("delete"), sendEvent: event => calls.push(event),
  };
  const transport = {
    resolve: id => ({ id, handle }),
    get: id => { calls.push(["get", id]); return { id, handle }; },
    create: body => { calls.push(["create", body]); return { id: body.id, handle }; },
    createBatch: batch => batch.map(body => ({ id: body.id, handle })),
    deleteBatch: ids => ({ deleted: ids.map(id => ({ id })), errors: [] }),
  };
  const binding = new WorkflowBinding(transport);
  const instance = await binding.get("order");
  assert.deepEqual(Object.keys(instance), ["id"]);
  assert.equal(JSON.stringify(instance), '{"id":"order"}');
  assert.deepEqual(await instance.status(), { status: "queued" });
  instance.pause(); instance.resume(); instance.restart(); instance.terminate();
  instance.sendEvent({ type: "approval", payload: { z: 1, a: 2 } });
  assert.deepEqual(calls, [["get", "order"], "pause", "resume", "restart", "terminate",
    { type: "approval", payloadBase64: encodeWorkflowBase64({ z: 1, a: 2 }) }]);
  assert.throws(() => instance.sendEvent({type: "bad type"}), /WORKFLOW_EVENT_TYPE_INVALID/);
  assert.throws(() => instance.restart({force:true}), /WORKFLOW_METHOD_UNSUPPORTED/);
  assert.throws(() => instance.sendEvent({type:"x",payload:"x".repeat(1024*1024)}), /WORKFLOW_PAYLOAD_TOO_LARGE/);
  const created = await binding.create({id:"fresh",retention:{successRetention:"1 hour"}});
  assert.equal(created.id, "fresh");
  assert.deepEqual(calls.at(-1), ["create", {id:"fresh",payloadBase64:encodeWorkflowBase64(undefined),retention:{successRetention:"1 hour"}}]);
  const generated = await binding.create({params:null});
  assert.match(generated.id, /^[0-9a-f-]{36}$/);
  assert.deepEqual(calls.at(-1), ["create", {id:generated.id,payloadBase64:encodeWorkflowBase64(null)}]);
  assert.deepEqual(await binding.deleteBatch(["order", "order"]), {
    deleted: [{ id: "order" }, { id: "order" }], errors: [],
  });
});

test("DO mutations require an installed output gate and keep readonly get/status", async () => {
  const fail = () => { throw new Error("transport mutation reached"); };
  const binding = new WorkflowBinding({
    resolve: id => ({id, handle:{status:()=>({status:"queued"}),pause:fail,resume:fail,terminate:fail,restart:fail,delete:fail,sendEvent:fail}}),
    create: fail,
    createBatch: fail, deleteBatch: fail,
    get: id => ({id, handle:{status:()=>({status:"queued"}),pause:fail,resume:fail,terminate:fail,restart:fail,delete:fail,sendEvent:fail}}),
  }, true, "FLOW");
  const instance = await binding.get("order");
  assert.deepEqual(await instance.status(), {status:"queued"});
  await assert.rejects(binding.create({ id: "x" }), error => {
    assert.match(String(error.message), /WORKFLOW_INVARIANT_VIOLATION/);
    return true;
  });
  assert.throws(() => instance.pause(), /WORKFLOW_INVARIANT_VIOLATION/);
});

test("direct Workflow schedules use one deterministic instance and operation per logical slot", async () => {
  const creates = [];
  const binding = new WorkflowBinding({
    resolve: id => ({ id, handle: {} }),
    get: id => ({ id, handle: {} }),
    create: (body, operationId) => { creates.push({ body, operationId }); return { id: body.id, handle: {} }; },
    createBatch: () => [],
    deleteBatch: () => ({ deleted: [], errors: [] }),
  }, false, "FLOW");
  const schedule = { cron: "*/5 * * * *", scheduledTime: 1_788_048_000_000 };
  await triggerWorkflowSchedule(binding, schedule);
  await triggerWorkflowSchedule(binding, schedule);
  assert.equal(creates.length, 2);
  assert.deepEqual(creates[0], creates[1]);
  assert.match(creates[0].body.id, /^schedule-[0-9a-f]{64}$/);
  assert.match(creates[0].operationId, /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
  assert.deepEqual(creates[0].body, {
    id: creates[0].body.id,
    payloadBase64: encodeWorkflowBase64(undefined),
    schedule,
  });
  await assert.rejects(
    triggerWorkflowSchedule({}, schedule),
    /WORKFLOW_RUNTIME_UNAVAILABLE/,
  );
  await assert.rejects(
    triggerWorkflowSchedule(binding, { cron: "", scheduledTime: schedule.scheduledTime }),
    /WORKFLOW_METHOD_UNSUPPORTED/,
  );
});
