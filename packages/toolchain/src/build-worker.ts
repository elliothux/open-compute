import { build } from "rolldown";
import { execFile } from "node:child_process";
import { realpath, stat } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, isAbsolute, relative, resolve, sep } from "node:path";

/** Module representation accepted by the canonical Rust bundle encoder. */
export type CompiledModuleType = "esModule" | "commonJsModule" | "text" | "json" | "data" | "wasm";

/** Module bytes ready for the canonical Rust bundle encoder. */
export interface CompiledModule {
  readonly name: string;
  readonly type: CompiledModuleType;
  readonly bytes: Uint8Array;
}

/** Immutable compiler output; it contains no deployment credentials or secrets. */
export interface CompiledWorker {
  readonly mainModule: string;
  readonly modules: readonly CompiledModule[];
}

/** Explicit project inputs shared by type checking and bundling. */
export interface WorkerBuildOptions {
  readonly project: string;
  readonly entry: string;
  readonly tsconfig: string;
}

async function projectFile(project: string, path: string, label: string): Promise<string> {
  const file = await realpath(resolve(project, path));
  const within = relative(project, file);
  if (within === ".." || within.startsWith(`..${sep}`) || isAbsolute(within)) {
    throw new Error(`${label} must be inside the project`);
  }
  if (!(await stat(file)).isFile()) throw new Error(`${label} must be a regular file`);
  return file;
}

function checkTypes(config: string, entry: string, cwd: string): Promise<void> {
  const require = createRequire(import.meta.url);
  const compiler = resolve(dirname(require.resolve("typescript/package.json")), "bin/tsc");
  return new Promise((accept, reject) => {
    // Explicit strict sub-options prevent tsconfig overrides from silently
    // weakening validation. No compiler output, including build caches, is written.
    const args = [compiler, "--project", config, "--noEmit", "--pretty", "false", "--listFiles",
      "--noCheck", "false", "--skipLibCheck", "false", "--incremental", "false", "--composite", "false",
      "--strict", "--noImplicitAny", "--noImplicitThis", "--strictNullChecks", "--strictFunctionTypes",
      "--strictBindCallApply", "--strictPropertyInitialization", "--strictBuiltinIteratorReturn",
      "--useUnknownInCatchVariables", "--alwaysStrict", "--isolatedModules", "--verbatimModuleSyntax",
      "--customConditions", "workerd,worker,browser"];
    execFile(process.execPath, args, {
      cwd, encoding: "utf8", maxBuffer: 8 * 1024 * 1024, timeout: 120_000, killSignal: "SIGKILL",
    }, (error, stdout, stderr) => {
      const lines = stdout.split(/\r?\n/).filter(Boolean);
      if (error) {
        const diagnostics = lines.filter(line => !isAbsolute(line)).join("\n");
        reject(new Error(`Worker TypeScript validation failed\n${diagnostics}\n${stderr}`.trim(), { cause: error }));
      } else if (!lines.some(line => isAbsolute(line) && resolve(line) === entry)) {
        reject(new Error("Worker entry is excluded from type checking"));
      } else accept();
    });
  });
}

/** Type-check and bundle a project without executing its code or installing dependencies. */
export async function compileWorker(options: WorkerBuildOptions): Promise<CompiledWorker> {
  if (!isAbsolute(options.project)) throw new Error("project path must be absolute");
  const project = await realpath(options.project);
  const entry = await projectFile(project, options.entry, "entry");
  const config = await projectFile(project, options.tsconfig, "tsconfig");
  await checkTypes(config, entry, project);
  const output = await build({
    cwd: project,
    input: { worker: entry },
    tsconfig: config,
    platform: "browser",
    preserveEntrySignatures: "strict",
    resolve: { conditionNames: ["workerd", "worker", "browser"] },
    external(id) {
      if (id === "cloudflare:workers" || id === "cloudflare:workflows" || id === "cloudflare:sockets") return true;
      if (id.startsWith("cloudflare:")) throw new Error("unsupported Cloudflare module");
      if (id.startsWith("node:")) return true;
      if (/^https?:/.test(id)) throw new Error("remote module imports are unsupported");
      return false;
    },
    onwarn(warning) {
      throw new Error(`Worker build warning: ${warning.code}`);
    },
    output: {
      format: "esm",
      entryFileNames: "worker.js",
      chunkFileNames: "modules/[name]-[hash].js",
      sourcemap: false,
    },
    write: false,
  });
  const modules: CompiledModule[] = [];
  let mainModule: string | undefined;
  for (const chunk of output.output) {
    if (chunk.type !== "chunk") throw new Error("unsupported emitted Worker resource");
    if (chunk.isEntry) {
      if (mainModule !== undefined) throw new Error("Worker must have one entry module");
      mainModule = chunk.fileName;
    }
    modules.push({ name: chunk.fileName, type: "esModule", bytes: new TextEncoder().encode(chunk.code) });
  }
  if (mainModule === undefined) throw new Error("Worker build did not emit an entry module");
  return { mainModule, modules };
}
