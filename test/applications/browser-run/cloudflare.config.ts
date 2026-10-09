import { bindings, defineConfig } from "cf/config";

export default defineConfig({
  worker: {
    name: "browser-run-fixture",
    entrypoint: "./src/index.ts",
    compatibilityDate: "2026-09-08",
    compatibilityFlags: ["nodejs_compat"],
    workersDev: false,
    env: { BROWSER: bindings.browser() },
  },
});
