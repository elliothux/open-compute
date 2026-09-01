import { DurableObject, RpcTarget, WorkerEntrypoint } from "cloudflare:workers";
import { WorkflowBinding } from "./workflows/facade.js";
import { prepareDurableObjectContext, activateDurableObjectAlarm } from "./durable-objects/alarm-shim.js";
import { runWithOutputGate } from "./durable-objects/output-gate.js";

function assert(value, message) { if (!value) throw new Error(message); }

const INDEX = {
  async upsert() {},
  async delete() {},
  async clear() {},
};

export class Store extends DurableObject {
  async create(id) {
    const count = (await this.ctx.storage.get("count")) || 0;
    await this.ctx.storage.put({ count: count + 1, [id]: true });
    return { id };
  }
  async get(id) { assert(await this.ctx.storage.get(id), "missing instance"); }
  async peek(id) { return this.ctx.storage.get(id); }
  async mark(id) { await this.ctx.storage.put(id, true); return true; }
  async status(id) { await this.get(id); return { status: "queued" }; }
  async count() { return (await this.ctx.storage.get("count")) || 0; }
}

class Handle extends RpcTarget {
  constructor(env, id) { super(); this.env = env; this.id = id; }
  status() { return this.env.STORE.getByName("authority").status(this.id); }
}

export class Backend extends WorkerEntrypoint {
  async create({ id }) {
    if (id === "crash-once" && !await this.env.STORE.getByName("authority").peek("crash-flag")) {
      await this.env.STORE.getByName("authority").mark("crash-flag");
      throw new Error("crash-before-ack");
    }
    await this.env.STORE.getByName("authority").create(id);
    return { id, handle: new Handle(this.env, id) };
  }
  async get(id) {
    await this.env.STORE.getByName("authority").get(id);
    return { id, handle: new Handle(this.env, id) };
  }
}

export class Caller extends DurableObject {
  constructor(ctx, env) {
    const prepared = prepareDurableObjectContext(ctx, INDEX);
    super(prepared.context, env);
    this.prepared = prepared;
    this.flow = new WorkflowBinding(env.BACKEND, true, "FLOW");
    activateDurableObjectAlarm(prepared, { FLOW: this.flow });
  }
  async probe(gated) {
    return runWithOutputGate(this.prepared.gate, async () => {
      const workflow = gated ? this.flow : new WorkflowBinding(this.env.BACKEND, false, "FLOW");
      let resolved = false;
      let pending;
      let code;
      try {
        await this.ctx.storage.transaction(async txn => {
          await txn.put("local", "must-rollback");
          pending = workflow.create({ id: gated ? "blocked" : "unsafe" });
          pending.then(() => { resolved = true; }, () => undefined);
          throw new Error("deliberate transaction abort");
        });
      } catch (error) { code = error.message; }
      assert(await this.ctx.storage.get("local") === undefined, "DO local mutation did not roll back");
      if (!gated) {
        await pending;
        resolved = true;
      }
      return { resolved, code };
    });
  }
  async commitOnce() {
    return runWithOutputGate(this.prepared.gate, async () => {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("committed", true);
        this.flow.create({ id: "once" }).catch(() => undefined);
      });
      return { stored: await this.ctx.storage.get("committed") === true };
    });
  }
  async commitCrash() {
    return runWithOutputGate(this.prepared.gate, async () => {
      await this.ctx.storage.transaction(async txn => {
        await txn.put("crash", true);
        this.flow.create({ id: "crash-once" }).catch(() => undefined);
      });
      return { stored: await this.ctx.storage.get("crash") === true };
    });
  }
  async recoverNow() {
    await this.prepared.gate.recover({ FLOW: this.flow });
    return true;
  }
  async crashStored() {
    return await this.ctx.storage.get("crash") === true;
  }
  async read() {
    const instance = await this.flow.get("ordinary");
    return instance.status();
  }
}

export default {
  async test(controller, env) {
    const workflow = new WorkflowBinding(env.BACKEND);
    const ordinary = await workflow.create({ id: "ordinary", params: { value: 1 } });
    assert(ordinary.id === "ordinary", "ordinary create failed");
    const caller = env.PROBES.getByName("caller");
    const ungated = await caller.probe(false);
    assert(ungated.resolved, "probe no longer observes external commit before DO abort");
    assert((await (await workflow.get("unsafe")).status()).status === "queued", "external mutation disappeared");
    const before = await env.STORE.getByName("authority").count();
    const gated = await caller.probe(true);
    assert(!gated.resolved, "gated mutation resolved before rollback");
    assert(await env.STORE.getByName("authority").count() === before, "rolled-back mutation changed authority");
    const committed = await caller.commitOnce();
    assert(committed.stored, "committed local write missing");
    assert(await env.STORE.getByName("authority").count() === before + 1, "commit did not publish once");
    let crashCode;
    try { await caller.commitCrash(); }
    catch (error) { crashCode = String(error && (error.stableCode || error.message) || error); }
    assert(crashCode === "DO_OUTPUT_GATE_PUBLISH_FAILED", "failed publish did not fail closed");
    assert(await caller.crashStored(), "crash local write missing");
    assert(await env.STORE.getByName("authority").count() === before + 1, "failed publish leaked an extra mutation");
    await caller.recoverNow();
    assert(await env.STORE.getByName("authority").count() === before + 2, "recover did not publish the committed intent once");
    assert((await caller.read()).status === "queued", "DO readonly get/status unavailable");
  },
};
