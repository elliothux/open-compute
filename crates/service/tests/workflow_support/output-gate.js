import { DurableObject, RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { WorkflowBinding } from "./workflow-facade.js";

function assert(value, message) { if (!value) throw new Error(message); }

export class Store extends DurableObject {
  async create(id) {
    const count = (await this.ctx.storage.get("count")) || 0;
    await this.ctx.storage.put({ count: count + 1, [id]: true });
    return { id };
  }
  async get(id) { assert(await this.ctx.storage.get(id), "missing instance"); }
  async status(id) { await this.get(id); return { status: "queued" }; }
  async count() { return (await this.ctx.storage.get("count")) || 0; }
}

class Handle extends RpcTarget {
  constructor(env, id) { super(); this.env = env; this.id = id; }
  status() { return this.env.STORE.getByName("authority").status(this.id); }
}

export class Backend extends WorkerEntrypoint {
  async create({ id }) {
    await this.env.STORE.getByName("authority").create(id);
    return { id, handle: new Handle(this.env, id) };
  }
  async get(id) {
    await this.env.STORE.getByName("authority").get(id);
    return { id, handle: new Handle(this.env, id) };
  }
}

export class Caller extends DurableObject {
  async probe(blocked) {
    const workflow = new WorkflowBinding(this.env.BACKEND, blocked);
    let resolved = false;
    let code;
    try {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("local", "must-rollback");
        await workflow.create({ id: blocked ? "blocked" : "unsafe" });
        resolved = true;
        throw new Error("deliberate transaction abort");
      });
    } catch (error) { code = error.message; }
    assert(await this.ctx.storage.get("local") === undefined, "DO local mutation did not roll back");
    return { resolved, code };
  }
  async read() {
    const workflow = new WorkflowBinding(this.env.BACKEND, true);
    const instance = await workflow.get("ordinary");
    return instance.status();
  }
}

export default {
  async test(controller, env) {
    const workflow = new WorkflowBinding(env.BACKEND);
    const ordinary = await workflow.create({ id: "ordinary", params: { value: 1 } });
    assert(ordinary.id === "ordinary", "ordinary create failed");
    const caller = env.PROBES.getByName("caller");
    const unsafe = await caller.probe(false);
    assert(unsafe.resolved, "probe no longer observes external commit before DO abort");
    assert((await (await workflow.get("unsafe")).status()).status === "queued", "external mutation disappeared");
    const before = await env.STORE.getByName("authority").count();
    const blocked = await caller.probe(true);
    assert(!blocked.resolved && blocked.code === "WORKFLOW_DO_OUTPUT_GATE_UNSUPPORTED", "DO mutation did not fail closed");
    assert(await env.STORE.getByName("authority").count() === before, "blocked mutation changed authority");
    assert((await caller.read()).status === "queued", "DO readonly get/status unavailable");
  },
};
