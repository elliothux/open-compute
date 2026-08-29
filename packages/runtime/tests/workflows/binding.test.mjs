import test from "node:test";
import assert from "node:assert/strict";
import { compileRuntime, importRuntime, moduleUrl } from "../compiled-runtime.mjs";

const codec = moduleUrl(await compileRuntime("workflows/json.ts"));
const { WorkflowBinding } = await importRuntime("workflows/facade.ts", {
  "./json.js": codec,
});

test("durable facade resolves once and keeps only a private instance-scoped handle", async () => {
  const calls = [];
  const handle = {
    status: () => ({ status: "queued" }),
    pause: () => calls.push("pause"), resume: () => calls.push("resume"),
    terminate: () => calls.push("terminate"), restart: () => calls.push("restart"),
    sendEvent: event => calls.push(event),
  };
  const binding = new WorkflowBinding({
    get: id => { calls.push(["get", id]); return { id, handle }; },
    create: body => { calls.push(["create", body]); return { id: body.id ?? "generated", handle }; },
  });
  const instance = await binding.get("order");
  assert.deepEqual(Object.keys(instance), ["id"]);
  assert.equal(JSON.stringify(instance), '{"id":"order"}');
  assert.deepEqual(await instance.status(), { status: "queued" });
  instance.pause(); instance.resume(); instance.restart(); instance.terminate();
  instance.sendEvent({ type: "approval", payload: { z: 1, a: 2 } });
  assert.deepEqual(calls, [["get", "order"], "pause", "resume", "restart", "terminate",
    { type: "approval", payloadJson: '{"a":2,"z":1}' }]);
  assert.throws(() => instance.sendEvent({type: "bad type"}), /WORKFLOW_EVENT_TYPE_INVALID/);
  assert.throws(() => instance.restart({force:true}), /WORKFLOW_METHOD_UNSUPPORTED/);
  assert.throws(() => instance.sendEvent({type:"x",payload:"x".repeat(1024*1024)}), /WORKFLOW_PAYLOAD_TOO_LARGE/);
  const created = await binding.create({id:"fresh",retention:{successRetention:"1 hour"}});
  assert.equal(created.id, "fresh");
  assert.deepEqual(calls.at(-1), ["create", {id:"fresh",payloadJson:"null",retention:{successRetention:"1 hour"}}]);
  const generated = await binding.create({params:null});
  assert.equal(generated.id, "generated");
  assert.deepEqual(calls.at(-1), ["create", {payloadJson:"null"}]);
});

test("DO reads remain available while every mutation fails before transport", async () => {
  const fail = () => { throw new Error("transport mutation reached"); };
  const binding = new WorkflowBinding({
    create: fail,
    get: id => ({id, handle:{status:()=>({status:"queued"}),pause:fail,resume:fail,terminate:fail,restart:fail,sendEvent:fail}}),
  }, true);
  const instance = await binding.get("order");
  assert.deepEqual(await instance.status(), {status:"queued"});
  await assert.rejects(binding.create(), /WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED/);
  for (const method of ["pause","resume","terminate","restart","sendEvent"]) {
    assert.throws(() => instance[method](), /WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED/);
  }
});
