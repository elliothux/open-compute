import { readFile, readdir, lstat, open, rename, mkdir, rm } from "node:fs/promises";
import { createHash, randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { resolve, dirname } from "node:path";
import { spawnSync } from "node:child_process";
import { transform } from "rolldown/utils";

const root = fileURLToPath(new URL("./", import.meta.url));
let check = false;
let outputDirectory = resolve(root, "system-workers");
let explicitOutput = false;
for (let index = 2; index < process.argv.length; index++) {
  const arg = process.argv[index];
  if (arg === "--check" && !check) check = true;
  else if (arg === "--output-dir" && !explicitOutput) {
    const path = process.argv[++index];
    if (!path || path.startsWith("--")) throw new Error("--output-dir requires a directory");
    outputDirectory = resolve(path);
    explicitOutput = true;
  } else throw new Error(`unexpected runtime build argument: ${arg}`);
}
const compiler = resolve(root, "../node_modules/.bin/tsc");
const checked = spawnSync(compiler, ["--project", resolve(root, "tsconfig.json"), "--noEmit"], {
  stdio: "inherit",
});
if (checked.error || checked.status !== 0) throw new Error("runtime TypeScript validation failed");

async function filesIn(directory: string, label: string, prefix = ""): Promise<string[]> {
  const files: string[] = [];
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const name = `${prefix}${entry.name}`;
    if (entry.isDirectory()) {
      files.push(...await filesIn(resolve(directory, entry.name), label, `${name}/`));
    } else if (entry.isFile()) files.push(name);
    else throw new Error(`runtime ${label} must be a regular file: ${name}`);
  }
  return files.sort();
}

const sources = (await filesIn(resolve(root, "src"), "source"))
  .filter(name => name.endsWith(".ts") && !name.endsWith(".d.ts"))
  .sort();
if (sources.length === 0) throw new Error("runtime sources are missing");
const emitted = new Map<string, string>();
for (const name of sources) {
  const sourcePath = resolve(root, "src", name);
  if (!(await lstat(sourcePath)).isFile()) throw new Error("runtime source must be a regular file");
  const source = await readFile(sourcePath, "utf8");
  const result = await transform(name, source, {
    target: "esnext", sourcemap: false, tsconfig: resolve(root, "tsconfig.json"),
  });
  if (result.errors.length || result.warnings.length) throw new Error(`runtime transform failed: ${name}`);
  const outputName = name.replace(/\.ts$/, ".js");
  const output = `// Generated from runtime/src/${name} by Rolldown. Do not edit.\n${result.code}`;
  emitted.set(outputName, output);
}
emitted.set("manifest.json", `${JSON.stringify({
  schemaVersion: 1,
  sources: Object.fromEntries([...emitted].map(([name, output]) => [name, createHash("sha256").update(output).digest("hex")])),
}, null, 2)}\n`);

// Compile and validate the complete asset set before replacing any existing file.
if (!check) await mkdir(outputDirectory, { recursive: true });
if (!(await lstat(outputDirectory)).isDirectory()) throw new Error("runtime output must be a regular directory");
const existing = await filesIn(outputDirectory, "asset");
for (const name of existing) {
  if (!emitted.has(name)) throw new Error(`unexpected runtime asset: ${name}`);
  if (!(await lstat(resolve(outputDirectory, name))).isFile()) {
    throw new Error(`runtime asset must be a regular file: ${name}`);
  }
}
const staged = new Map<string, string>();
try {
  for (const [name, output] of emitted) {
    const outputPath = resolve(outputDirectory, name);
    if (check) {
      if (await readFile(outputPath, "utf8") !== output) throw new Error(`stale runtime asset: ${name}`);
    } else {
      await mkdir(dirname(outputPath), { recursive: true });
      const temporary = `${outputPath}.${randomUUID()}.tmp`;
      const file = await open(temporary, "wx", 0o644);
      staged.set(temporary, outputPath);
      try { await file.writeFile(output); }
      finally { await file.close(); }
    }
  }
  for (const [temporary, outputPath] of staged) await rename(temporary, outputPath);
} finally {
  for (const temporary of staged.keys()) await rm(temporary, { force: true });
}
