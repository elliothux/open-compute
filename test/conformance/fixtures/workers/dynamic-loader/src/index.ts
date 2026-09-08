import { WorkerEntrypoint } from "cloudflare:workers";

interface Env {
  LOADER: WorkerLoader;
}

export class ScopedService extends WorkerEntrypoint<Env, { prefix: string }> {
  override fetch(request: Request): Response {
    return Response.json({
      prefix: this.ctx.props.prefix,
      path: new URL(request.url).pathname,
    });
  }
}

function code(
  modules: WorkerLoaderWorkerCode["modules"],
): WorkerLoaderWorkerCode {
  return {
    compatibilityDate: "2026-08-30",
    mainModule: "main.js",
    modules,
    globalOutbound: null,
  };
}

async function result(stub: WorkerStub, name?: string): Promise<unknown> {
  return (
    await stub.getEntrypoint(name).fetch("https://dynamic.invalid/check")
  ).json();
}

async function rejected(
  operation: () => unknown | Promise<unknown>,
): Promise<boolean> {
  try {
    await operation();
    return false;
  } catch {
    return true;
  }
}

export default {
  async fetch(
    _request: Request,
    env: Env,
    ctx: ExecutionContext,
  ): Promise<Response> {
    const exports = ctx.exports as typeof ctx.exports & {
      ScopedService: LoopbackServiceStub<ScopedService>;
    };
    const scope = crypto.randomUUID();
    const basic = code({
      "main.js": `
      let count = 0;
      export default { fetch() { return Response.json({ value: "child", count: ++count }); } };
      export const named = { fetch() { return Response.json({ value: "named" }); } };
    `,
    });
    const first = env.LOADER.load(basic);
    const second = env.LOADER.load(basic);
    const named = env.LOADER.get(`${scope}/named`, async () => basic);
    const output: Record<string, unknown> = {
      synchronousStub: typeof first.getEntrypoint === "function",
      unnamed: [await result(first), await result(second)],
      namedEntrypoint: await result(named, "named"),
      missingEntrypointRejected: await rejected(() => result(named, "missing")),
      emptyModulesRejected: await rejected(() =>
        result(env.LOADER.load(code({}))),
      ),
      missingMainRejected: await rejected(() =>
        result(env.LOADER.load(code({ "other.js": "export default {}" }))),
      ),
    };
    const moduleCode = code({
      "main.js": `
        import text from "./text.txt";
        import data from "./data.bin";
        import json from "./value.json";
        import common from "./common.cjs";
        export default { fetch() { return Response.json({ text, data: [...new Uint8Array(data)], json, common }); } };
      `,
      "text.txt": { text: "text" },
      "data.bin": { data: new Uint8Array([1, 2, 3]).buffer },
      "value.json": { json: { value: 7 } },
      "common.cjs": { cjs: "module.exports = 'common';" },
    });
    output.modules = await result(env.LOADER.load(moduleCode));
    output.callbackRejected = await rejected(() =>
      result(
        env.LOADER.get(`${scope}/retry`, async () => {
          throw new Error("fixture callback rejection");
        }),
      ),
    );
    output.callbackRetry = await result(
      env.LOADER.get(`${scope}/retry`, () => basic),
    );

    const envCode = code({
      "main.js": `
      export default { fetch(request, env) {
        return Response.json({ keys: Object.keys(env).sort(), value: env.value, nested: env.nested });
      } };
    `,
    });
    envCode.env = { value: "visible", nested: { list: [1, true, null] } };
    output.env = await result(env.LOADER.load(envCode));

    const blockedCode = code({
      "main.js": `
      import { connect } from "cloudflare:sockets";
      export default { async fetch() {
        let fetchBlocked = false, connectBlocked = false;
        try { await fetch("https://example.com"); } catch { fetchBlocked = true; }
        try { const socket = connect("example.com:443"); await socket.opened; await socket.close(); }
        catch { connectBlocked = true; }
        return Response.json({ fetchBlocked, connectBlocked });
      } };
    `,
    });
    output.nullOutbound = await result(env.LOADER.load(blockedCode));

    const proxyCode = code({
      "main.js": `
      export default { fetch() { return fetch("https://proxy.invalid/scoped"); } };
    `,
    });
    proxyCode.globalOutbound = exports.ScopedService({
      props: { prefix: "allowed" },
    });
    output.redirectedOutbound = await result(env.LOADER.load(proxyCode));

    const capabilityCode = code({
      "main.js": `
      export default { fetch(request, env) { return env.SERVICE.fetch("https://service.invalid/resource"); } };
    `,
    });
    capabilityCode.env = {
      SERVICE: exports.ScopedService({ props: { prefix: "scoped" } }),
    };
    output.serviceCapability = await result(env.LOADER.load(capabilityCode));
    return Response.json(output);
  },
} satisfies ExportedHandler<Env>;
