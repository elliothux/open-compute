import {
  compileRuntime,
  importRuntime,
  moduleUrl,
} from "../compiled-runtime.mjs";

const workers = moduleUrl(`
  export class WorkerEntrypoint {
    constructor(ctx, env) { this.ctx = ctx; this.env = env; }
  }
`);
const tunnel = moduleUrl(await compileRuntime("sockets/tunnel.ts"));
export const { default: Ingress } = await importRuntime("gateway/ingress.ts", {
  "cloudflare:workers": workers,
  "../sockets/tunnel.js": tunnel,
});
