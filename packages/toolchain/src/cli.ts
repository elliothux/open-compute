import { createHash } from "node:crypto";
import { mkdir, open, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";
import { parseArgs } from "node:util";
import { compileWorker } from "./build-worker.ts";
import { encodeWorker } from "./bundle-worker.ts";
import { readAssetObject, scanAssets } from "./assets/scan.ts";
import type { ScannedAssets } from "./assets/types.ts";
import { deployProject } from "./deploy-worker.ts";
import { generateEnvTypes, writeGeneratedTypes } from "./generate-types.ts";
import { loadProject } from "./project.ts";
import { applyFrameworkOutput, importFrameworkOutput, type FrameworkOutput } from "./import/framework-output.ts";
import type { WorkerProject } from "./project.ts";

const HELP = `Usage: oc <build|run|deploy|types> [entry.ts] [options]

  build    Type-check and write a canonical Worker bundle without network access
  run      Compile and serve a Worker through an already-running local platform
  deploy   Compile, validate, and activate a Worker on the configured platform
  types    Generate Env types from the project configuration without ocd

Options:
  --config <file>       Project JSON (default: open-compute.json)
  --ocd <file>          Matching ocd binary for Worker code, or OPEN_COMPUTE_OCD
  --out <file>          New bundle for build, or types destination for types
  --endpoint <origin>   Override the platform origin
  --account <id>        Override the platform's default account
  --token-env <name>    Admin token environment variable (default: OPEN_COMPUTE_ADMIN_TOKEN)
  --json               Emit public result metadata as JSON
  --help               Show this help

Dependencies must already be installed. No runtime or package is downloaded.
types is offline and does not encode Worker code or contact the platform.
run activates a deployment on the local platform; it does not start another workerd.
`;

function hasOption(args: readonly string[], name: string): boolean {
  const flag = `--${name}`;
  return args.some(arg => arg === flag || arg.startsWith(`${flag}=`));
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

/** Execute a developer command without loading executable project configuration. */
export async function runCli(args: readonly string[]): Promise<void> {
  const { values, positionals } = parseArgs({
    args: [...args], allowPositionals: true, strict: true,
    options: {
      config: { type: "string", default: "open-compute.json" },
      ocd: { type: "string" }, out: { type: "string" },
      endpoint: { type: "string" }, account: { type: "string" },
      "token-env": { type: "string", default: "OPEN_COMPUTE_ADMIN_TOKEN" },
      json: { type: "boolean", default: false }, help: { type: "boolean", default: false },
    },
  });
  if (values.help) { process.stdout.write(HELP); return; }
  const [command, entry] = positionals;
  if (command === "types") {
    if (positionals.length !== 1) throw new Error("types does not accept an entry argument");
    for (const name of ["ocd", "endpoint", "account", "token-env", "json"] as const) {
      if (hasOption(args, name)) throw new Error(`types does not accept --${name}`);
    }
    const { project } = await configuredProject(values.config);
    const output = values.out === undefined
      ? join(project.project, "worker-configuration.d.ts")
      : resolve(values.out);
    const written = await writeGeneratedTypes(output, generateEnvTypes(project, output));
    process.stdout.write(`Wrote ${written}\n`);
    return;
  }
  if (!["build", "run", "deploy"].includes(command ?? "") || positionals.length > 2) throw new Error(HELP);
  if (command === "build" && values.out === undefined) throw new Error("build requires --out; existing output is never overwritten");
  if (command !== "build" && values.out !== undefined) throw new Error("--out is only supported by build");
  const { project, framework } = await configuredProject(values.config);
  if (entry !== undefined && project.frameworkOutput !== undefined) {
    throw new Error("an entry argument cannot override frameworkOutput");
  }
  const main = entry ?? project.main;
  const binary = values.ocd ?? process.env.OPEN_COMPUTE_OCD;
  if (main !== undefined && !binary) throw new Error("set --ocd or OPEN_COMPUTE_OCD to encode Worker code");
  const assets = project.assets === undefined
    ? undefined : await scanAssets(project.project, project.assets);
  let artifact: Awaited<ReturnType<typeof encodeWorker>> | undefined;
  if (framework !== undefined) {
    if (binary === undefined) throw new Error("ocd is required to encode framework Worker code");
    artifact = await encodeWorker(framework.worker, resolve(binary));
  } else if (main !== undefined) {
    if (binary === undefined) throw new Error("ocd is required to encode Worker code");
    artifact = await encodeWorker(await compileWorker({
      project: project.project, entry: main, tsconfig: project.tsconfig,
    }), resolve(binary));
  }
  if (command === "build") {
    if (values.out === undefined) throw new Error("build requires --out");
    const output = resolve(values.out);
    await mkdir(dirname(output), { recursive: true });
    const bytes = assets === undefined
      ? artifact?.bytes
      : await deploymentPackage(artifact, assets);
    if (bytes === undefined) throw new Error("build has no deployment content");
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    const file = await open(output, "wx", 0o600);
    try { await file.writeFile(bytes); await file.sync(); }
    catch (error) { await file.close(); await rm(output); throw error; }
    await file.close();
    process.stdout.write(values.json
      ? `${JSON.stringify({ output, sha256, bytes: bytes.byteLength })}\n`
      : `Built ${output} (${bytes.byteLength} bytes, SHA-256 ${sha256})\n`);
    return;
  }
  const token = process.env[values["token-env"]];
  const result = await deployProject(project, artifact, assets, {
    localOnly: command === "run",
    ...(values.endpoint === undefined ? {} : { endpoint: values.endpoint }),
    ...(values.account === undefined ? {} : { accountId: values.account }),
    ...(token === undefined ? {} : { token }),
  });
  process.stdout.write(values.json ? `${JSON.stringify(result)}\n` : `Worker is serving at ${result.url}\nDeployment: ${result.deploymentId}\n`);
}

async function deploymentPackage(
  artifact: Awaited<ReturnType<typeof encodeWorker>> | undefined,
  assets: ScannedAssets,
): Promise<Uint8Array> {
  const objects: { sha256: string; size: number; bytesBase64: string }[] = [];
  for (const source of [...assets.objects.values()].sort((left, right) => left.sha256.localeCompare(right.sha256))) {
    const bytes = await readAssetObject(source);
    objects.push({ sha256: source.sha256, size: source.size, bytesBase64: Buffer.from(bytes).toString("base64") });
  }
  return Buffer.from(JSON.stringify({
    schemaVersion: 1,
    contentKind: artifact === undefined ? "assets_only" : "worker",
    ...(artifact === undefined ? {} : {
      bundle: { sha256: artifact.sha256, size: artifact.bytes.byteLength,
        bytesBase64: Buffer.from(artifact.bytes).toString("base64") },
    }),
    manifest: assets.manifest,
    routing: assets.routing,
    objects,
  }));
}
