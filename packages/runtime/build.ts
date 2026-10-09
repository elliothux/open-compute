import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  lstat,
  mkdir,
  open,
  readdir,
  readFile,
  rename,
  rm,
} from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "rolldown";
import { transform } from "rolldown/utils";

const root = fileURLToPath(new URL("./", import.meta.url));
let check = false;
let outputDirectory = resolve(root, "dist");
let explicitOutput = false;
for (let index = 2; index < process.argv.length; index++) {
  const arg = process.argv[index];
  if (arg === "--check" && !check) check = true;
  else if (arg === "--output-dir" && !explicitOutput) {
    const path = process.argv[++index];
    if (!path || path.startsWith("--"))
      throw new Error("--output-dir requires a directory");
    outputDirectory = resolve(path);
    explicitOutput = true;
  } else throw new Error(`unexpected runtime build argument: ${arg}`);
}
if (!check) {
  const compiler = resolve(root, "../../node_modules/.bin/tsc");
  for (const config of ["tsconfig.json", "tsconfig.build.json"]) {
    const checked = spawnSync(
      compiler,
      ["--project", resolve(root, config), "--noEmit"],
      {
        stdio: "inherit",
      },
    );
    if (checked.error || checked.status !== 0)
      throw new Error("runtime TypeScript validation failed");
  }
}

if (!check) {
  const checked = spawnSync(
    resolve(root, "../../node_modules/.bin/tsc"),
    [
      "--project",
      resolve(root, "../browser-actions/tsconfig.json"),
      "--noEmit",
    ],
    { stdio: "inherit" },
  );
  if (checked.error || checked.status !== 0)
    throw new Error("browser actions TypeScript validation failed");
}

async function filesIn(
  directory: string,
  label: string,
  prefix = "",
): Promise<string[]> {
  const files: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const name = `${prefix}${entry.name}`;
    if (entry.isDirectory()) {
      files.push(
        ...(await filesIn(resolve(directory, entry.name), label, `${name}/`)),
      );
    } else if (entry.isFile()) files.push(name);
    else throw new Error(`runtime ${label} must be a regular file: ${name}`);
  }
  return files.sort();
}

const inputs = [
  "bun.lock",
  "third_party/workerd/src/cloudflare/workers.ts",
  "third_party/workerd/src/cloudflare/workflows.ts",
  "third_party/workerd/src/node/async_hooks.ts",
  "package.json",
  "tsconfig.json",
  "packages/browser-actions/package.json",
  "packages/browser-actions/tsconfig.json",
  ...(
    await filesIn(
      resolve(root, "../browser-actions/src"),
      "browser actions source",
    )
  ).map((name) => `packages/browser-actions/src/${name}`),
  ...["build.ts", "package.json", "tsconfig.json", "tsconfig.build.json"].map(
    (name) => `packages/runtime/${name}`,
  ),
  ...(await filesIn(resolve(root, "src"), "source")).map(
    (name) => `packages/runtime/src/${name}`,
  ),
].sort();
const inputDigests = Object.fromEntries(
  await Promise.all(
    inputs.map(async (name) => [
      name,
      createHash("sha256")
        .update(await readFile(resolve(root, "../..", name)))
        .digest("hex"),
    ]),
  ),
);
const sources = (await filesIn(resolve(root, "src"), "source"))
  .filter((name) => name.endsWith(".ts") && !name.endsWith(".d.ts"))
  .sort();
if (sources.length === 0) throw new Error("runtime sources are missing");
const emitted = new Map<string, string>();
for (const name of sources) {
  const sourcePath = resolve(root, "src", name);
  if (!(await lstat(sourcePath)).isFile())
    throw new Error("runtime source must be a regular file");
  const source = await readFile(sourcePath, "utf8");
  let code: string;
  if (
    name === "loader/host-policy.ts" ||
    name === "loader/forwarding.ts" ||
    name === "ai-search/namespace-binding.ts" ||
    name === "ai-search/instance-binding.ts"
  ) {
    // Private entrypoints bundle their local closure; only workerd builtins remain imports.
    const result = await build({
      cwd: root,
      input: sourcePath,
      external: (id) =>
        id.startsWith("cloudflare-internal:") ||
        id.startsWith("node-internal:"),
      resolve: {
        alias: {
          "cloudflare:workers": resolve(
            root,
            "../../third_party/workerd/src/cloudflare/workers.ts",
          ),
          "cloudflare:workflows": resolve(
            root,
            "../../third_party/workerd/src/cloudflare/workflows.ts",
          ),
          "node:async_hooks": resolve(
            root,
            "../../third_party/workerd/src/node/async_hooks.ts",
          ),
        },
      },
      tsconfig: resolve(root, "tsconfig.json"),
      output: { format: "esm", sourcemap: false, codeSplitting: false },
      write: false,
      onwarn(warning) {
        throw new Error(
          `runtime binding bundle failed: ${name}: ${warning.code}`,
        );
      },
    });
    const chunk = result.output[0];
    if (result.output.length !== 1 || chunk?.type !== "chunk")
      throw new Error(`runtime binding must produce one module: ${name}`);
    code = chunk.code;
  } else {
    const result = await transform(name, source, {
      target: "esnext",
      sourcemap: false,
      tsconfig: resolve(root, "tsconfig.json"),
    });
    if (result.errors.length || result.warnings.length)
      throw new Error(`runtime transform failed: ${name}`);
    code = result.code;
  }
  const outputName = name.replace(/\.ts$/, ".js");
  const output = `// Generated from packages/runtime/src/${name} by Rolldown. Do not edit.\n${code}`;
  emitted.set(outputName, output);
}
const actions = await build({
  cwd: root,
  input: resolve(root, "../browser-actions/src/index.ts"),
  external: (id) => id === "cloudflare:workers" || id.startsWith("node:"),
  tsconfig: resolve(root, "../browser-actions/tsconfig.json"),
  output: { format: "esm", sourcemap: false, codeSplitting: false },
  write: false,
  onwarn(warning) {
    throw new Error(`browser actions bundle failed: ${warning.code}`);
  },
});
const actionChunk = actions.output[0];
if (actions.output.length !== 1 || actionChunk?.type !== "chunk")
  throw new Error("browser actions must produce one module");
emitted.set(
  "browser/actions.js",
  `// Generated from packages/browser-actions/src/index.ts by Rolldown. Do not edit.\n${actionChunk.code}`,
);

const viewer = await build({
  cwd: root,
  input: resolve(root, "../browser-actions/src/live-view.ts"),
  tsconfig: resolve(root, "../browser-actions/tsconfig.json"),
  output: { format: "esm", sourcemap: false, codeSplitting: false },
  write: false,
  onwarn(warning) {
    throw new Error(`browser view bundle failed: ${warning.code}`);
  },
});
const viewChunk = viewer.output[0];
if (viewer.output.length !== 1 || viewChunk?.type !== "chunk")
  throw new Error("browser view must produce one module");
emitted.set(
  "browser/live-view.js",
  `// Generated from packages/browser-actions/src/live-view.ts. Do not edit.\n${viewChunk.code}`,
);

emitted.set(
  "manifest.json",
  `${JSON.stringify(
    {
      schemaVersion: 1,
      inputs: inputDigests,
      sources: Object.fromEntries(
        [...emitted]
          .sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))
          .map(([name, output]) => [
            name,
            createHash("sha256").update(output).digest("hex"),
          ]),
      ),
    },
    null,
    2,
  )}\n`,
);

// Compile and validate the complete asset set before replacing any existing file.
if (!check) await mkdir(outputDirectory, { recursive: true });
if (!(await lstat(outputDirectory)).isDirectory())
  throw new Error("runtime output must be a regular directory");
const existing = await filesIn(outputDirectory, "asset");
const obsolete = existing.filter((name) => !emitted.has(name));
if (obsolete.length) {
  const unexpected = () =>
    new Error(`unexpected runtime asset: ${obsolete[0]}`);
  if (check) throw unexpected();
  let previous: unknown;
  try {
    previous = JSON.parse(
      await readFile(resolve(outputDirectory, "manifest.json"), "utf8"),
    );
  } catch {
    throw unexpected();
  }
  if (
    previous === null ||
    typeof previous !== "object" ||
    Array.isArray(previous) ||
    !("schemaVersion" in previous) ||
    previous.schemaVersion !== 1 ||
    !("sources" in previous) ||
    previous.sources === null ||
    typeof previous.sources !== "object" ||
    Array.isArray(previous.sources)
  )
    throw unexpected();
  for (const name of obsolete) {
    const output = await readFile(resolve(outputDirectory, name));
    const digest = createHash("sha256").update(output).digest("hex");
    const header = `// Generated from packages/runtime/src/${name.replace(/\.js$/, ".ts")} by Rolldown. Do not edit.\n`;
    if (
      !name.endsWith(".js") ||
      !Object.prototype.hasOwnProperty.call(previous.sources, name) ||
      Reflect.get(previous.sources, name) !== digest ||
      !output.subarray(0, Buffer.byteLength(header)).equals(Buffer.from(header))
    )
      throw unexpected();
  }
}
for (const name of existing) {
  if (!(await lstat(resolve(outputDirectory, name))).isFile()) {
    throw new Error(`runtime asset must be a regular file: ${name}`);
  }
}
const staged = new Map<string, string>();
try {
  for (const [name, output] of emitted) {
    const outputPath = resolve(outputDirectory, name);
    if (check) {
      if ((await readFile(outputPath, "utf8")) !== output)
        throw new Error(`stale runtime asset: ${name}`);
    } else {
      // Identical builds must not invalidate Cargo's asset mtimes.
      if (
        existing.includes(name) &&
        (await readFile(outputPath, "utf8")) === output
      )
        continue;
      await mkdir(dirname(outputPath), { recursive: true });
      const temporary = `${outputPath}.${randomUUID()}.tmp`;
      const file = await open(temporary, "wx", 0o644);
      staged.set(temporary, outputPath);
      try {
        await file.writeFile(output);
      } finally {
        await file.close();
      }
    }
  }
  // Only unchanged files owned by the previous generated manifest may be retired.
  for (const name of obsolete) await rm(resolve(outputDirectory, name));
  for (const [temporary, outputPath] of staged)
    await rename(temporary, outputPath);
} finally {
  for (const temporary of staged.keys()) await rm(temporary, { force: true });
}
