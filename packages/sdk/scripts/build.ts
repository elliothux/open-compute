import { spawnSync } from "node:child_process";
import { copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "rolldown";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const dist = resolve(root, "dist");
const tsc = resolve(root, "../../node_modules/.bin/tsc");

function runTool(command: string, args: string[]): void {
  const result = spawnSync(command, args, { stdio: "inherit", cwd: root });
  if (result.status !== 0)
    throw new Error(`${command} ${args.join(" ")} failed`);
}

await rm(dist, { recursive: true, force: true });
await mkdir(dist, { recursive: true });
runTool(tsc, ["--project", resolve(root, "tsconfig.json"), "--noEmit"]);
await build([
  {
    input: { index: resolve(root, "src/index.ts") },
    platform: "node",
    external: [/^cloudflare($|\/)/],
    output: { format: "esm", file: resolve(dist, "index.mjs") },
  },
  {
    input: { index: resolve(root, "src/index.ts") },
    platform: "node",
    external: [/^cloudflare($|\/)/],
    output: { format: "cjs", file: resolve(dist, "index.cjs") },
  },
]);
runTool(tsc, ["--project", resolve(root, "tsconfig.build.json")]);
// The pinned TypeScript emits `.ts` relative specifiers verbatim; rewrite them
// so the published declarations resolve under both module systems.
for (const name of ["index.d.ts", "client.d.ts", "generated.d.ts"]) {
  const path = resolve(dist, name);
  const source = await readFile(path, "utf8");
  await writeFile(
    path,
    source.replaceAll(/(from|import)(\s*")(\.\/[^"]*)\.ts(")/g, "$1$2$3.js$4"),
  );
}
await copyFile(resolve(dist, "index.d.ts"), resolve(dist, "index.d.mts"));
console.log("built packages/sdk/dist");
