import { build } from "rolldown";

await build({
  input: { oc: "src/bin.ts", "build-worker": "src/build-worker.ts" },
  platform: "node",
  external: ["rolldown"],
  preserveEntrySignatures: "strict",
  output: { dir: "dist", format: "esm", entryFileNames: "[name].js" },
});
