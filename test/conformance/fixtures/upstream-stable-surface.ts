interface Env {
  KV: KVNamespace;
  BUCKET: R2Bucket;
  DB: D1Database;
  OBJECTS: DurableObjectNamespace;
  QUEUE: Queue;
  WORKFLOW: Workflow;
}

export default {
  async fetch(request: Request, env: Env, ctx: ExecutionContext): Promise<Response> {
    const _exports = ctx.exports;
    const _props = ctx.props;
    const cached = await caches.default.match(request);
    const values = await env.KV.get(["a", "b"]);
    const upload = env.BUCKET.createMultipartUpload("object");
    const session = env.DB.withSession();
    const id = env.OBJECTS.newUniqueId();
    const stub = env.OBJECTS.get(id);
    await env.QUEUE.send({ ok: true });
    const instance = await env.WORKFLOW.create();
    const rewritten = new HTMLRewriter().transform(new Response("body"));
    return cached ?? rewritten ?? new Response(JSON.stringify({
      values: values.size,
      upload: typeof upload.then,
      session: typeof session.prepare,
      stub: stub.id.toString(),
      instance: instance.id,
      exports: _exports !== undefined,
      props: _props,
    }));
  },
} satisfies ExportedHandler<Env>;
