import { WorkflowEntrypoint, DurableObject } from 'cloudflare:workers';

const VERSION = 'frozen';

export class Counter extends DurableObject {
  async inspectWorkflow(id) {
    const handle = await this.env.FLOW.get(id);
    const actions = [
      () => this.env.FLOW.create({ id: `from-object-${id}` }),
      () => handle.pause(),
      () => handle.resume(),
      () => handle.terminate(),
      () => handle.restart(),
      () => handle.sendEvent({ type: 'blocked', payload: null }),
    ];
    const errors = [];
    for (const action of actions) {
      try { await action(); errors.push('accepted'); }
      catch (error) { errors.push(error.message); }
    }
    return { id: handle.id, status: (await handle.status()).status, errors };
  }
  async recordOnce(key) {
    const prior = await this.ctx.storage.get('effect');
    if (prior) return prior;
    await this.ctx.storage.put('effect', { key, count: 1 });
    return { key, count: 1 };
  }
  async arm(key) {
    await this.ctx.storage.put('alarm-key', key);
    await this.ctx.storage.setAlarm(Date.now() + 2000);
    return true;
  }
  async alarm() {
    await this.env.KV.put(`alarm:${await this.ctx.storage.get('alarm-key')}`, 'done');
  }
}

export class Flow extends WorkflowEntrypoint {
  async run(event, step) {
    const id = event.payload.id;
    let callbacks = 0;
    const products = await step.do('products', async () => {
      callbacks++;
      let stage = 'kv';
      try {
        await this.env.KV.put(`product:${id}`, VERSION);
        stage = 'r2';
        await this.env.R2.put(id, VERSION);
        stage = 'd1';
        await this.env.DB.exec('CREATE TABLE IF NOT EXISTS effects (id TEXT PRIMARY KEY, value TEXT NOT NULL)');
        await this.env.DB.prepare('INSERT OR IGNORE INTO effects VALUES (?, ?)').bind(id, VERSION).run();
        stage = 'readback-and-do';
        return {
          version: VERSION,
          kv: await this.env.KV.get(`product:${id}`),
          r2: await (await this.env.R2.get(id)).text(),
          d1: await this.env.DB.prepare('SELECT value FROM effects WHERE id=?').bind(id).first('value'),
          object: await this.env.OBJECTS.getByName(id).recordOnce(id),
        };
      } catch (error) {
        // Retain only a stage and stable code in this tenant-owned fixture.
        // A diagnostic never substitutes for the failed operation or its retry.
        const code = /^([A-Z][A-Z0-9_]{1,63})(?::|$)/.exec(error.message)?.[1] || 'UNKNOWN';
        await this.env.KV.put(`diagnostic:${id}`, JSON.stringify({ stage, code }));
        throw error;
      }
    });
    await step.do('uncommitted', { timeout: '60 seconds' }, async context => {
      await this.env.KV.put(`entered:${id}`, String(context.attempt));
      while (!(await this.env.KV.get(`release-workflow:${id}`))) {
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      return { attempt: context.attempt };
    });
    await step.sleep('wake', '5 seconds');
    const signal = await step.waitForEvent('handoff', { type: 'continue', timeout: '10 minutes' });
    return { version: VERSION, products, callbacks, payload: signal.payload, dated: signal.timestamp instanceof Date };
  }
}

export default {
  async fetch(request, env) {
    const [operation, id] = new URL(request.url).pathname.split('/').filter(Boolean);
    if (!operation) return new Response('ready');
    if (operation === 'enqueue') {
      const receipt = await env.QUEUE.send({ id }, { delaySeconds: 2 });
      return Response.json({ accepted: true, receipt });
    }
    if (operation === 'release-consumer' || operation === 'release-workflow') {
      await env.KV.put(`${operation}:${id}`, 'yes');
      return Response.json({ ok: true });
    }
    if (operation === 'arm') {
      await env.OBJECTS.getByName(id).arm(id);
      return Response.json({ ok: true });
    }
    if (operation === 'guards') return Response.json(await env.OBJECTS.getByName(id).inspectWorkflow(id));
    if (operation === 'diagnostic') return Response.json(await env.KV.get(`diagnostic:${id}`, 'json'));
    if (operation === 'effects') {
      return Response.json({
        kv: await env.KV.get(`product:${id}`),
        r2: await (await env.R2.get(id)).text(),
        rows: await env.DB.prepare('SELECT count(*) AS count FROM effects WHERE id=?').bind(id).first('count'),
        object: await env.OBJECTS.getByName(id).recordOnce(id),
        alarm: await env.KV.get(`alarm:${id}`),
      });
    }
    const handle = await env.FLOW.get(id);
    if (operation === 'status') return Response.json(await handle.status());
    if (operation === 'event') await handle.sendEvent(await request.json());
    else if (operation === 'pause') await handle.pause();
    else if (operation === 'resume') await handle.resume();
    else return new Response('not found', { status: 404 });
    return Response.json({ ok: true });
  },
  async queue(batch, env) {
    for (const message of batch.messages) {
      let handle;
      try {
        handle = await env.FLOW.create({ id: message.body.id, params: message.body });
      } catch (error) {
        if (error.message !== 'WORKFLOW_INSTANCE_ALREADY_EXISTS') throw error;
        handle = await env.FLOW.get(message.body.id);
      }
      await handle.status();
      while (!(await env.KV.get(`release-consumer:${message.body.id}`))) {
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      message.ack();
    }
  },
};
