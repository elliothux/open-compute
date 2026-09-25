import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseSdkPackageReport } from "../../../scripts/assemble-release.ts";

const packageRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageRoot, "../..");
const work = resolve(repoRoot, ".temp/sdk-package");

function run(
  command: string,
  args: string[],
  options: { cwd?: string; capture?: boolean } = {},
): { status: number; stdout: string; stderr: string } {
  const result = spawnSync(command, args, {
    cwd: options.cwd ?? packageRoot,
    encoding: "utf8",
    stdio: options.capture ? "pipe" : "inherit",
  });
  if (result.error !== undefined && result.error !== null) throw result.error;
  return {
    status: result.status ?? 1,
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
  };
}

function requireRun(
  command: string,
  args: string[],
  options: { cwd?: string; capture?: boolean } = {},
): string {
  const result = run(command, args, options);
  if (result.status !== 0)
    throw new Error(
      `${command} ${args.join(" ")} failed: ${result.stderr}${result.stdout}`,
    );
  return result.stdout;
}

function sha(content: Buffer, algorithm: "sha1" | "sha512"): string {
  return createHash(algorithm)
    .update(content)
    .digest(algorithm === "sha1" ? "hex" : "base64");
}

rmSync(work, { recursive: true, force: true });
mkdirSync(work, { recursive: true });
const packOutput = requireRun("bun", ["pm", "pack"], { capture: true });
const tarballName = packOutput
  .trim()
  .split("\n")
  .filter((line) => line.trim().endsWith(".tgz"))
  .at(-1);
if (tarballName === undefined)
  throw new Error(`bun pm pack produced no tarball: ${packOutput}`);
// bun pm pack writes next to the package root regardless of process cwd.
const packedPath = resolve(packageRoot, tarballName);
const tarballPath = resolve(work, tarballName);
cpSync(packedPath, tarballPath);
rmSync(packedPath);
const tarball = readFileSync(tarballPath);
const tarballShasum = sha(tarball, "sha1");
const tarballIntegrity = `sha512-${sha(tarball, "sha512")}`;

const listOutput = requireRun("tar", ["-tzf", tarballPath], {
  cwd: work,
  capture: true,
});
const files = listOutput
  .trim()
  .split("\n")
  .filter((line) => line.trim().length > 0)
  .sort();
const allowed = new Set([
  "package/package.json",
  "package/README.md",
  "package/LICENSE",
  "package/dist/index.mjs",
  "package/dist/index.cjs",
  "package/dist/index.d.ts",
  "package/dist/index.d.mts",
  "package/dist/client.d.ts",
  "package/dist/generated.d.ts",
  "package/dist/artifacts.d.ts",
  "package/dist/ai-search-upload.d.ts",
  "package/dist/worker-settings-edit.d.ts",
]);
const unexpected = files.filter((file) => !allowed.has(file));
if (unexpected.length > 0)
  throw new Error(
    `tarball contains unexpected files: ${unexpected.join(", ")}`,
  );
const missing = [...allowed].filter((file) => !files.includes(file));
if (missing.length > 0)
  throw new Error(`tarball is missing files: ${missing.join(", ")}`);

requireRun("tar", ["-xzf", tarballPath], { cwd: work, capture: true });
const packedPackageJson = JSON.parse(
  readFileSync(resolve(work, "package/package.json"), "utf8"),
);
const packageJson = JSON.parse(
  readFileSync(resolve(packageRoot, "package.json"), "utf8"),
);
const lock = JSON.parse(
  readFileSync(
    resolve(repoRoot, "openapi/upstream/cloudflare-openapi.lock.json"),
    "utf8",
  ),
);
const surface = JSON.parse(
  readFileSync(resolve(packageRoot, "surface.json"), "utf8"),
);
if (packedPackageJson.name !== "@open-compute/sdk")
  throw new Error("packed package name mismatch");
if (packedPackageJson.version !== packageJson.version)
  throw new Error("packed package version mismatch");
if (packedPackageJson.license !== "Apache-2.0")
  throw new Error("packed package license mismatch");
if (packedPackageJson.sideEffects !== false)
  throw new Error("packed package must declare sideEffects: false");
if (packedPackageJson.publishConfig?.access !== "public")
  throw new Error("packed package must publish with public access");
if (packedPackageJson.dependencies?.cloudflare !== lock.cloudflareSdk.version)
  throw new Error(
    `packed dependency must be exactly cloudflare@${lock.cloudflareSdk.version}`,
  );
for (const [key, value] of Object.entries(packageJson.exports ?? {})) {
  if (
    JSON.stringify(packedPackageJson.exports?.[key]) !== JSON.stringify(value)
  )
    throw new Error(`packed exports drift for ${key}`);
}

const forbidden =
  /NPM_ACCESS_TOKEN|BEGIN (RSA |EC |OPENSSH )?PRIVATE KEY|\.npmrc/;
for (const file of files) {
  const content = readFileSync(resolve(work, file), "utf8");
  if (forbidden.test(content))
    throw new Error(`tarball file ${file} contains forbidden material`);
}

// Isolated consumer smoke: install the packed tarball alone and exercise the
// public surface from Node ESM, Node CommonJS, Bun, and TypeScript.
const consumer = resolve(work, "consumer");
mkdirSync(consumer, { recursive: true });
writeFileSync(
  resolve(consumer, "package.json"),
  `${JSON.stringify({ name: "sdk-consumer-smoke", private: true, type: "module" }, null, 2)}\n`,
);
writeFileSync(
  resolve(consumer, "index.mjs"),
  `import { createOpenComputeClient } from "@open-compute/sdk";
const client = createOpenComputeClient({
  apiToken: "smoke-token",
  baseURL: "http://127.0.0.1:18787/client/v4",
});
const keys = Object.keys(client).sort().join(",");
if (!keys.includes("openCompute") || !keys.includes("workers"))
  throw new Error("unexpected runtime surface: " + keys);
console.log("esm:" + keys);
`,
);
writeFileSync(
  resolve(consumer, "index.cjs"),
  `const { createOpenComputeClient } = require("@open-compute/sdk");
const client = createOpenComputeClient({
  apiToken: "smoke-token",
  baseURL: "http://127.0.0.1:18787/client/v4",
});
if (typeof client.openCompute.system.status !== "function")
  throw new Error("missing vendor method over require()");
console.log("cjs:ok");
`,
);
writeFileSync(
  resolve(consumer, "bun.ts"),
  `import { createOpenComputeClient, APIError } from "@open-compute/sdk";
const client = createOpenComputeClient({
  apiToken: "smoke-token",
  baseURL: "http://127.0.0.1:18787/client/v4",
});
if (typeof client.d1.database.list !== "function") throw new Error("missing d1");
const error: APIError | null = null;
if (error !== null) throw error;
console.log("bun:ok");
`,
);
writeFileSync(
  resolve(consumer, "types.ts"),
  `import type { createOpenComputeClient } from "@open-compute/sdk";
type Client = ReturnType<typeof createOpenComputeClient>;
type Probe = Client["openCompute"]["backups"]["kv"]["restore"];
const probe: Probe | null = null;
if (probe !== null) throw probe;
console.log("types:ok");
`,
);
requireRun("npm", ["install", "--no-audit", "--no-fund", tarballPath], {
  cwd: consumer,
  capture: true,
});
const installed = JSON.parse(
  readFileSync(
    resolve(consumer, "node_modules/@open-compute/sdk/package.json"),
    "utf8",
  ),
);
if (installed.dependencies?.cloudflare !== lock.cloudflareSdk.version)
  throw new Error(
    `consumer resolved cloudflare@${installed.dependencies?.cloudflare}; expected ${lock.cloudflareSdk.version}`,
  );
requireRun("node", ["index.mjs"], { cwd: consumer, capture: true });
requireRun("node", ["index.cjs"], { cwd: consumer, capture: true });
requireRun("bun", ["bun.ts"], { cwd: consumer, capture: true });
const tsc = resolve(repoRoot, "node_modules/.bin/tsc");
requireRun(
  tsc,
  [
    "--noEmit",
    "--ignoreConfig",
    "--strict",
    "--target",
    "es2024",
    "--module",
    "preserve",
    "--moduleResolution",
    "bundler",
    "types.ts",
  ],
  { cwd: consumer, capture: true },
);

const report = parseSdkPackageReport({
  schemaVersion: 1,
  package: packedPackageJson.name,
  packageVersion: packedPackageJson.version,
  tarball: tarballName,
  tarballShasum,
  tarballIntegrity,
  surfaceDigest: surface.surfaceDigest,
  openapiRevision: lock.revision,
  cloudflareSdkVersion: lock.cloudflareSdk.version,
  files,
});
console.log(`${JSON.stringify(report, null, 2)}\n`);
writeFileSync(
  resolve(work, "sdk-package-report.json"),
  `${JSON.stringify(report, null, 2)}\n`,
);
