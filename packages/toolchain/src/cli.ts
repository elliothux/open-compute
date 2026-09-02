import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdir, open, rm } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { scanAssets } from "./assets/scan.ts";
import { compileWorker } from "./build-worker.ts";
import { encodeWorker } from "./bundle-worker.ts";
import { generateEnvTypes, writeGeneratedTypes } from "./generate-types.ts";
import { applyFrameworkOutput, importFrameworkOutput, type FrameworkOutput } from "./import/framework-output.ts";
import { loadProject, type WorkerProject } from "./project.ts";

const HELP = `Usage: oc <build|types|deploy|run> [options]

  build    Type-check and write a canonical Worker bundle without network access
  types    Generate Env types from wrangler.jsonc without contacting the platform
  deploy   Invoke the exact pinned upstream Wrangler deploy command
  run      Invoke the exact pinned upstream Wrangler deploy command

Local build/types options:
  --config <file>  Wrangler config (default: wrangler.jsonc)
  --ocd <file>     Matching ocd binary for Worker code, or OPEN_COMPUTE_OCD
  --out <file>     New bundle for build, or types destination for types
  --json           Emit build result metadata as JSON
  --help           Show this help

deploy/run pass all remaining arguments directly to wrangler@4.127.1.
Authentication and the API origin use CLOUDFLARE_API_TOKEN,
CLOUDFLARE_ACCOUNT_ID, and CLOUDFLARE_API_BASE_URL.
`;

const require = createRequire(import.meta.url);

/** Resolve the JavaScript entrypoint of the directly pinned Wrangler package. */
export function wranglerEntrypoint(): string {
  const packagePath = require.resolve("wrangler/package.json");
  return resolve(dirname(packagePath), "bin/wrangler.js");
}

/** Map online convenience commands to the sole upstream deployment transport. */
export function wranglerArgs(args: readonly string[]): string[] {
  const [command, ...rest] = args;
  if (command === "deploy" || command === "run") return ["deploy", ...rest];
  throw new Error("only deploy and run are Wrangler transport commands");
}

async function runWrangler(args: readonly string[]): Promise<void> {
  const child = spawn(process.execPath, [wranglerEntrypoint(), ...wranglerArgs(args)], {
    env: process.env,
    stdio: "inherit",
  });
  const code = await new Promise<number>((resolveCode, reject) => {
    child.once("error", reject);
    child.once("exit", (status, signal) => {
      if (signal !== null) reject(new Error(`Wrangler terminated by ${signal}`));
      else resolveCode(status ?? 1);
    });
  });
  if (code !== 0) throw new Error(`Wrangler exited with status ${code}`);
}

async function configuredProject(config: string): Promise<{
  project: WorkerProject;
  framework: FrameworkOutput | undefined;
}> {
  const loaded = await loadProject(config);
  if (loaded.frameworkOutput === undefined) return { project: loaded, framework: undefined };
  const framework = await importFrameworkOutput(loaded);
  return { project: applyFrameworkOutput(loaded, framework), framework };
}

/** Execute local build/typegen or delegate online deployment to pinned Wrangler. */
export async function runCli(args: readonly string[]): Promise<void> {
  const [command] = args;
  if (command === undefined || command === "--help" || command === "-h") {
    process.stdout.write(HELP);
    return;
  }
  if (command === "deploy" || command === "run") {
    await runWrangler(args);
    return;
  }
  const { values, positionals } = parseArgs({
    args: [...args],
    allowPositionals: true,
    strict: true,
    options: {
      config: { type: "string", default: "wrangler.jsonc" },
      ocd: { type: "string" },
      out: { type: "string" },
      json: { type: "boolean", default: false },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) {
    process.stdout.write(HELP);
    return;
  }
  if ((command !== "build" && command !== "types") || positionals.length !== 1) throw new Error(HELP);
  const { project, framework } = await configuredProject(values.config);
  if (command === "types") {
    if (values.ocd !== undefined || values.json) throw new Error("types accepts only --config and --out");
    const output = values.out === undefined
      ? join(project.project, "worker-configuration.d.ts")
      : resolve(values.out);
    const written = await writeGeneratedTypes(output, generateEnvTypes(project, output));
    process.stdout.write(`Wrote ${written}\n`);
    return;
  }
  if (values.out === undefined) throw new Error("build requires --out; existing output is never overwritten");
  const binary = values.ocd ?? process.env.OPEN_COMPUTE_OCD;
  const main = project.main;
  if ((framework !== undefined || main !== undefined) && binary === undefined) {
    throw new Error("set --ocd or OPEN_COMPUTE_OCD to encode Worker code");
  }
  if (project.assets !== undefined) await scanAssets(project.project, project.assets);
  let artifact: Awaited<ReturnType<typeof encodeWorker>> | undefined;
  if (framework !== undefined && binary !== undefined) {
    artifact = await encodeWorker(framework.worker, resolve(binary));
  } else if (main !== undefined && binary !== undefined) {
    artifact = await encodeWorker(await compileWorker({
      project: project.project,
      entry: main,
      tsconfig: project.tsconfig,
    }), resolve(binary));
  }
  if (artifact === undefined) {
    throw new Error("assets-only projects deploy through pinned Wrangler; build requires a Worker module");
  }
  const bytes = artifact.bytes;
  const output = resolve(values.out);
  await mkdir(dirname(output), { recursive: true });
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const file = await open(output, "wx", 0o600);
  try {
    await file.writeFile(bytes);
    await file.sync();
  } catch (error) {
    await file.close();
    await rm(output);
    throw error;
  }
  await file.close();
  process.stdout.write(values.json
    ? `${JSON.stringify({ output, sha256, bytes: bytes.byteLength })}\n`
    : `Built ${output} (${bytes.byteLength} bytes, SHA-256 ${sha256})\n`);
}
