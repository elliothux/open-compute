import { build } from "rolldown";
import { spawnSync } from "node:child_process";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./", import.meta.url));
const check = process.argv.includes("--check");

if (!check) {
  const compiler = resolve(root, "../../node_modules/.bin/tsc");
  const checked = spawnSync(compiler, ["--project", resolve(root, "tsconfig.json"), "--noEmit"], {
    stdio: "inherit",
  });
  if (checked.error || checked.status !== 0) throw new Error("operator-sdk TypeScript validation failed");
  const typeTests = spawnSync(compiler, ["--project", resolve(root, "tsconfig.type-tests.json"), "--noEmit"], {
    stdio: "inherit",
  });
  if (typeTests.error || typeTests.status !== 0) throw new Error("operator-sdk type contract tests failed");
}

await build({
  input: {
    index: "src/index.ts",
    transport: "src/transport.ts",
    registry: "src/operations/registry.ts",
  },
  platform: "node",
  preserveEntrySignatures: "strict",
  output: {
    dir: "dist",
    format: "esm",
    entryFileNames: "[name].js",
  },
});

const dts = spawnSync(resolve(root, "../../node_modules/.bin/tsc"), [
  "--project",
  resolve(root, "tsconfig.build.json"),
  "--emitDeclarationOnly",
], { stdio: "inherit" });
if (dts.error || dts.status !== 0) throw new Error("operator-sdk declaration emit failed");
