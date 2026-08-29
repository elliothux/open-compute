import { readFile, readdir, lstat, open, rename, mkdir, rm } from "node:fs/promises";
import { createHash, randomUUID } from "node:crypto";
import { fileURLToPath } from "node:url";
import { resolve, dirname } from "node:path";
import { spawnSync } from "node:child_process";
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
    if (!path || path.startsWith("--")) throw new Error("--output-dir requires a directory");
    outputDirectory = resolve(path);
    explicitOutput = true;
  } else throw new Error(`unexpected runtime build argument: ${arg}`);
}
if (!check) {
  const compiler = resolve(root, "../../node_modules/.bin/tsc");
  for (const config of ["tsconfig.json", "tsconfig.build.json"]) {
    const checked = spawnSync(compiler, ["--project", resolve(root, config), "--noEmit"], {
      stdio: "inherit",
    });
    if (checked.error || checked.status !== 0) throw new Error("runtime TypeScript validation failed");
  }
}

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

const inputs = [
  "bun.lock", "package.json", "tsconfig.json",
  ...["build.ts", "package.json", "tsconfig.json", "tsconfig.build.json"].map(name => `packages/runtime/${name}`),
  ...(await filesIn(resolve(root, "src"), "source")).map(name => `packages/runtime/src/${name}`),
].sort();
const inputDigests = Object.fromEntries(await Promise.all(inputs.map(async name => [
  name, createHash("sha256").update(await readFile(resolve(root, "../..", name))).digest("hex"),
])));
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
  const output = `// Generated from packages/runtime/src/${name} by Rolldown. Do not edit.\n${result.code}`;
  emitted.set(outputName, output);
}
emitted.set("manifest.json", `${JSON.stringify({
  schemaVersion: 1,
  inputs: inputDigests,
  sources: Object.fromEntries([...emitted].map(([name, output]) => [name, createHash("sha256").update(output).digest("hex")])),
}, null, 2)}\n`);

// Compile and validate the complete asset set before replacing any existing file.
if (!check) await mkdir(outputDirectory, { recursive: true });
if (!(await lstat(outputDirectory)).isDirectory()) throw new Error("runtime output must be a regular directory");
const existing = await filesIn(outputDirectory, "asset");
const obsolete = existing.filter(name => !emitted.has(name));
if (obsolete.length) {
  const unexpected = () => new Error(`unexpected runtime asset: ${obsolete[0]}`);
  if (check) throw unexpected();
  let previous: unknown;
  try { previous = JSON.parse(await readFile(resolve(outputDirectory, "manifest.json"), "utf8")); }
  catch { throw unexpected(); }
  if (previous === null || typeof previous !== "object" || Array.isArray(previous)
      || !("schemaVersion" in previous) || previous.schemaVersion !== 1
      || !("sources" in previous) || previous.sources === null
      || typeof previous.sources !== "object" || Array.isArray(previous.sources)) throw unexpected();
  for (const name of obsolete) {
    const output = await readFile(resolve(outputDirectory, name));
    const digest = createHash("sha256").update(output).digest("hex");
    const header = `// Generated from packages/runtime/src/${name.replace(/\.js$/, ".ts")} by Rolldown. Do not edit.\n`;
    if (!name.endsWith(".js") || !Object.prototype.hasOwnProperty.call(previous.sources, name)
        || Reflect.get(previous.sources, name) !== digest
        || !output.subarray(0, Buffer.byteLength(header)).equals(Buffer.from(header))) throw unexpected();
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
      if (await readFile(outputPath, "utf8") !== output) throw new Error(`stale runtime asset: ${name}`);
    } else {
      // Identical builds must not invalidate Cargo's asset mtimes.
      if (existing.includes(name) && await readFile(outputPath, "utf8") === output) continue;
      await mkdir(dirname(outputPath), { recursive: true });
      const temporary = `${outputPath}.${randomUUID()}.tmp`;
      const file = await open(temporary, "wx", 0o644);
      staged.set(temporary, outputPath);
      try { await file.writeFile(output); }
      finally { await file.close(); }
    }
  }
  // Only unchanged files owned by the previous generated manifest may be retired.
  for (const name of obsolete) await rm(resolve(outputDirectory, name));
  for (const [temporary, outputPath] of staged) await rename(temporary, outputPath);
} finally {
  for (const temporary of staged.keys()) await rm(temporary, { force: true });
}
