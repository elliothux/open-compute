import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { generateBindingWrapper } from "../system-workers/loaded-isolate-wrapper-generator-v3.js";

const source = name => readFileSync(new URL(`../system-workers/${name}`, import.meta.url), "utf8");
const codec = new URL("../system-workers/workflow-json.js", import.meta.url).href;
const facade = source("workflow-facade-v2.js").replace("./__open_compute_workflow_json__.js", codec);
const { WorkflowBindingV2 } = await import(`data:text/javascript;base64,${Buffer.from(facade).toString("base64")}`);

test("earlier wrapper, runner and facade sources remain immutable", () => {
  const digests = {
    "loaded-isolate-wrapper-generator.js": "ca5fdd28cf500239102d2e4f83d4fae9ac96080a1bf199a900c17170389c1e08",
    "loaded-isolate-wrapper-generator-v2.js": "e74c516c4e9ef2c4ef858d91f232de59810c8e33696e28fd4f9e0963e7c990a5",
    "workflow-runner.js": "d0393966e178441ad3fd4a7cd2061162c3d4a1566f5e07f9eff0aa9a5c3ee9be",
    "workflow-json.js": "e40180928eb3f4611039a63960c610d8b21e358db0058b53805347cca6c81a66",
    "workflow-facade.js": "3d7cb193b3636b70bc635d4d53054f5c81e991e52012bb9eaa6532892909c94c",
  };
  for (const [name, expected] of Object.entries(digests)) {
    assert.equal(createHash("sha256").update(source(name)).digest("hex"), expected, name);
  }
  const wrapper = generateBindingWrapper("index.js", [], [], [], undefined, false, [], false, ["OLD"], "", ["NEW"]);
  assert.match(wrapper, /new WorkflowBinding\(out\[name\], false\)/);
  assert.match(wrapper, /new WorkflowBindingV2\(out\[name\], false\)/);
  assert.match(wrapper, /WORKFLOW_BINDINGS = \["OLD"\]/);
  assert.match(wrapper, /WORKFLOW_V2_BINDINGS = \["NEW"\]/);
});

test("V2 facade resolves once and keeps only a private instance-scoped handle", async () => {
  const calls = [];
  const handle = {
    status: () => ({ status: "queued" }),
    pause: () => calls.push("pause"), resume: () => calls.push("resume"),
    terminate: () => calls.push("terminate"), restart: () => calls.push("restart"),
    sendEvent: event => calls.push(event),
  };
  const binding = new WorkflowBindingV2({
    get: id => { calls.push(["get", id]); return { id, handle }; },
    create: body => { calls.push(["create", body]); return { id: body.id, handle }; },
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
});

test("DO reads remain available while every V2 mutation fails before transport", async () => {
  const fail = () => { throw new Error("transport mutation reached"); };
  const binding = new WorkflowBindingV2({
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
