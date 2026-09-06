/** Compile the public P1 Loader subset directly against the pinned upstream declarations. */
export function dynamicLoaderSurface(loader: WorkerLoader, tail: Fetcher, wasm: WebAssembly.Module) {
  const modules: WorkerLoaderWorkerCode["modules"] = {
    "main.js": { js: "export default { fetch() { return new Response('ok'); } };" },
    "common.cjs": { cjs: "module.exports = 'common';" },
    "message.txt": { text: "message" },
    "bytes.bin": { data: new Uint8Array([1, 2, 3]) },
    "config.json": { json: { enabled: true } },
    "python.py": { py: "value = 1" },
    "module.wasm": { wasm },
  };
  const code: WorkerLoaderWorkerCode = {
    compatibilityDate: "2026-08-30",
    compatibilityFlags: ["nodejs_compat"],
    mainModule: "main.js",
    modules,
    env: { value: "scoped", service: tail },
    globalOutbound: null,
    tails: [tail],
  };
  const named = loader.get("immutable-code", async () => code);
  const unnamed = loader.load(code);
  const options: WorkerStubEntrypointOptions = { props: { marker: "scoped" } };
  return {
    entrypoint: named.getEntrypoint("Named", options),
    defaultEntrypoint: unnamed.getEntrypoint(),
    actorClass: named.getDurableObjectClass("Child", options),
  };
}
