/** Compile the public P1 Loader subset directly against the pinned upstream declarations. */
export function dynamicLoaderSurface(
  loader: WorkerLoader,
  tail: Fetcher,
  wasm: WebAssembly.Module,
) {
  const modules: WorkerLoaderWorkerCode["modules"] = {
    "main.js": {
      js: "export default { fetch() { return new Response('ok'); } };",
    },
    "common.cjs": { cjs: "module.exports = 'common';" },
    "message.txt": { text: "message" },
    "bytes.bin": { data: new Uint8Array([1, 2, 3]) },
    "config.json": { json: { enabled: true } },
    "python.py": { py: "value = 1" },
    "module.wasm": { wasm },
  };
  const code: WorkerLoaderWorkerCode = {
    compatibilityDate: "2026-09-08",
    compatibilityFlags: ["nodejs_compat"],
    limits: { cpuMs: 30_000, subRequests: 10_000 },
    mainModule: "main.js",
    modules,
    env: { value: "scoped", service: tail },
    globalOutbound: null,
    tails: [tail],
  };
  // @ts-expect-error The private forwarding channel is not a Cloudflare WorkerCode field.
  code.openComputePrivateEnv;
  const named = loader.get("immutable-code", async () => code);
  const unnamed = loader.load(code);
  const options: WorkerStubEntrypointOptions = { props: { marker: "scoped" } };
  const limited: WorkerStubEntrypointOptions = {
    props: { marker: "narrower" },
    limits: { cpuMs: 1_000 },
  };
  return {
    entrypoint: named.getEntrypoint("Named", options),
    defaultEntrypoint: unnamed.getEntrypoint(),
    limitedEntrypoint: named.getEntrypoint("Named", limited),
    actorClass: named.getDurableObjectClass("Child", options),
    limitedActorClass: named.getDurableObjectClass("Child", limited),
  };
}
