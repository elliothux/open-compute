import { mkdir, open, rm } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { parseArgs } from "node:util";
import { compileWorker } from "./build-worker.ts";
import { encodeWorker } from "./bundle-worker.ts";
import { deployWorker } from "./deploy-worker.ts";
import { loadProject } from "./project.ts";

const HELP = `Usage: oc <build|run|deploy> [entry.ts] [options]

  build    Type-check and write a canonical Worker bundle without network access
  run      Compile and serve a Worker through an already-running local platform
  deploy   Compile, validate, and activate a Worker on the configured platform

Options:
  --config <file>       Project JSON (default: open-compute.json)
  --platformd <file>    Matching platformd binary, or OPEN_COMPUTE_PLATFORMD
  --out <file>          New bundle output file (required for build; never overwritten)
  --endpoint <origin>   Override the platform origin
  --account <id>        Override the platform's default account
  --token-env <name>    Admin token environment variable (default: OPEN_COMPUTE_ADMIN_TOKEN)
  --json               Emit public result metadata as JSON
  --help               Show this help

Dependencies must already be installed. No runtime or package is downloaded.
run activates a deployment on the local platform; it does not start another workerd.
`;

/** Execute a developer command without loading executable project configuration. */
export async function runCli(args: readonly string[]): Promise<void> {
  const { values, positionals } = parseArgs({
    args: [...args], allowPositionals: true, strict: true,
    options: {
      config: { type: "string", default: "open-compute.json" },
      platformd: { type: "string" }, out: { type: "string" },
      endpoint: { type: "string" }, account: { type: "string" },
      "token-env": { type: "string", default: "OPEN_COMPUTE_ADMIN_TOKEN" },
      json: { type: "boolean", default: false }, help: { type: "boolean", default: false },
    },
  });
  if (values.help) { process.stdout.write(HELP); return; }
  const [command, entry] = positionals;
  if (!["build", "run", "deploy"].includes(command ?? "") || positionals.length > 2) throw new Error(HELP);
  if (command === "build" && values.out === undefined) throw new Error("build requires --out; existing output is never overwritten");
  if (command !== "build" && values.out !== undefined) throw new Error("--out is only supported by build");
  const binary = values.platformd ?? process.env.OPEN_COMPUTE_PLATFORMD;
  if (!binary) throw new Error("set --platformd or OPEN_COMPUTE_PLATFORMD to a matching platformd binary");
  const project = await loadProject(values.config);
  const compiled = await compileWorker({
    project: project.project, entry: entry ?? project.main, tsconfig: project.tsconfig,
    compatibilityFlags: project.compatibilityFlags,
  });
  const artifact = await encodeWorker(compiled, resolve(binary));
  if (command === "build") {
    if (values.out === undefined) throw new Error("build requires --out");
    const output = resolve(values.out);
    await mkdir(dirname(output), { recursive: true });
    const file = await open(output, "wx", 0o600);
    try { await file.writeFile(artifact.bytes); await file.sync(); }
    catch (error) { await file.close(); await rm(output); throw error; }
    await file.close();
    process.stdout.write(values.json
      ? `${JSON.stringify({ output, sha256: artifact.sha256, bytes: artifact.bytes.byteLength })}\n`
      : `Built ${output} (${artifact.bytes.byteLength} bytes, SHA-256 ${artifact.sha256})\n`);
    return;
  }
  const token = process.env[values["token-env"]];
  const result = await deployWorker(project, artifact, {
    localOnly: command === "run",
    ...(values.endpoint === undefined ? {} : { endpoint: values.endpoint }),
    ...(values.account === undefined ? {} : { accountId: values.account }),
    ...(token === undefined ? {} : { token }),
  });
  process.stdout.write(values.json ? `${JSON.stringify(result)}\n` : `Worker is serving at ${result.url}\nDeployment: ${result.deploymentId}\n`);
}
