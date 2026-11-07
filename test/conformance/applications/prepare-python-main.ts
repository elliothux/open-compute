// Developer-only cf upload capture. Product Gates consume the resulting static bytes.
import { spawn } from "node:child_process";
import { mkdir, readFile, realpath, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { repository, sha256 } from "../../../scripts/workerd-archive.ts";
import { PythonAssetCapture } from "./python-assets-capture.ts";

const maxBody = 34 * 1024 * 1024;
const account = "019c0000-0000-7000-8000-000000000001";
const token = "python-fixture-capture-only";
type Fixture =
  | "main"
  | "django"
  | "flask"
  | "fastapi"
  | "services"
  | "queues"
  | "workflows"
  | "runtime"
  | "durableobjects"
  | "durableobjects-retired";

const scripts: Record<Fixture, string> = {
  main: "python-main-fixture",
  services: "python-services-fixture",
  queues: "python-queues-fixture",
  workflows: "python-workflows-fixture",
  runtime: "python-runtime-fixture",
  durableobjects: "python-durable-objects-fixture",
  "durableobjects-retired": "python-durable-objects-fixture",
  django: "python-django-fixture",
  flask: "python-flask-fixture",
  fastapi: "python-fastapi-fixture",
};

const packageFiles: Record<Fixture, string[]> = {
  main: ["greeting/__init__.py", "greeting/message.json"],
  services: [],
  queues: [],
  workflows: [],
  runtime: [
    "cache_cases.py",
    "image_cases.py",
    "ai_cases.py",
    "vector_cases.py",
    "artifact_cases.py",
    "search_cases.py",
    "http_client_cases.py",
  ],
  durableobjects: [],
  "durableobjects-retired": [],
  django: ["app_urls.py"],
  flask: ["templates/page.html"],
  fastapi: [],
};

export async function describePythonUpload(
  contentType: string,
  body: Uint8Array,
  fixture: Fixture = "main",
) {
  if (
    body.byteLength > maxBody ||
    !contentType.startsWith("multipart/form-data;")
  )
    throw new Error("expected bounded SDK multipart upload");
  const form = await new Request("http://127.0.0.1/upload", {
    method: "POST",
    headers: { "content-type": contentType },
    body: Buffer.from(body),
  }).formData();
  const names = new Set<string>();
  const modules: {
    name: string;
    uploadName: string;
    mime: string;
    size: number;
    sha256: string;
  }[] = [];
  let metadata: Record<string, unknown> | undefined;
  for (const [name, part] of form) {
    // cf prefixes non-entrypoint module names with ./ on the v4 upload wire.
    const canonicalName = name.replace(/^\.\//u, "");
    if (names.has(canonicalName)) throw new Error("duplicate multipart part");
    names.add(canonicalName);
    if (name === "metadata") {
      const value: unknown = JSON.parse(
        typeof part === "string" ? part : await part.text(),
      );
      if (typeof value !== "object" || value === null || Array.isArray(value))
        throw new Error("invalid SDK metadata");
      metadata = value as Record<string, unknown>;
    } else {
      if (
        typeof part === "string" ||
        part.name !== name ||
        canonicalName.split("/").some((p) => !p || p === "." || p === "..") ||
        canonicalName.includes("\\")
      )
        throw new Error("invalid SDK module part");
      const bytes = new Uint8Array(await part.arrayBuffer());
      modules.push({
        name: canonicalName,
        uploadName: name,
        mime: part.type,
        size: bytes.byteLength,
        sha256: sha256(bytes),
      });
    }
  }
  if (!metadata) throw new Error("missing SDK metadata");
  const main = metadata.main_module;
  if (
    typeof main !== "string" ||
    !main.endsWith(".py") ||
    !modules.some((m) => m.name === main && m.mime === "text/x-python")
  )
    throw new Error("missing typed Python main");
  if (
    packageFiles[fixture].some((name) => !modules.some((m) => m.name === name))
  )
    throw new Error("missing local Python package/data");
  if (
    !modules.some(
      (m) =>
        m.name.startsWith("python_modules/workers/") &&
        /\.m?js$/.test(m.name) &&
        m.mime === "application/javascript+module",
    )
  )
    throw new Error("missing SDK JavaScript bridge");
  if (
    metadata.compatibility_date !== "2026-09-08" ||
    !Array.isArray(metadata.compatibility_flags) ||
    !metadata.compatibility_flags.includes("python_workers")
  )
    throw new Error("unexpected Python compatibility inputs");
  modules.sort((a, b) => a.name.localeCompare(b.name));
  return {
    contentType,
    size: body.byteLength,
    sha256: sha256(body),
    metadata,
    modules,
  };
}

export function captureEnvironment(origin: string): NodeJS.ProcessEnv {
  const url = new URL(origin);
  if (
    url.protocol !== "http:" ||
    url.hostname !== "127.0.0.1" ||
    !url.port ||
    url.pathname !== "/" ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error("capture origin must be a loopback HTTP listener");
  const environment = { ...process.env };
  for (const key of [
    "CF_API_TOKEN",
    "CF_API_KEY",
    "CF_API_EMAIL",
    "CF_ACCOUNT_ID",
    "CF_API_BASE_URL",
    "CLOUDFLARE_API_KEY",
    "CLOUDFLARE_EMAIL",
    "CLOUDFLARE_API_USER_SERVICE_KEY",
    "CLOUDFLARE_BASE_URL",
  ])
    delete environment[key];
  return {
    ...environment,
    CLOUDFLARE_API_BASE_URL: `${origin}/client/v4`,
    CLOUDFLARE_API_TOKEN: token,
    CLOUDFLARE_ACCOUNT_ID: account,
    CF_SEND_TELEMETRY: "false",
    DO_NOT_TRACK: "1",
    CI: "true",
    NODE_DISABLE_COMPILE_CACHE: "1",
    BUN_RUNTIME_TRANSPILER_CACHE_PATH: "0",
    NO_PROXY: "127.0.0.1,localhost",
    UV_PYTHON_DOWNLOADS: "never",
    UV_OFFLINE: "true",
    UV_CACHE_DIR: join(repository, ".temp/uv-cache"),
  };
}

async function command(
  executable: string,
  args: string[],
  cwd: string,
  env: NodeJS.ProcessEnv,
) {
  const child = spawn(executable, args, {
    cwd,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  const chunks: Buffer[] = [];
  let size = 0;
  const capture = (chunk: Buffer) => {
    size += chunk.length;
    if (size > 1024 * 1024) child.kill("SIGKILL");
    else chunks.push(chunk);
  };
  child.stdout.on("data", capture);
  child.stderr.on("data", capture);
  const timer = setTimeout(() => child.kill("SIGKILL"), 60_000);
  try {
    const code = await new Promise<number | null>((resolve, reject) => {
      child.once("error", reject);
      child.once("close", resolve);
    });
    const output = Buffer.concat(chunks)
      .toString("utf8")
      .replaceAll(token, "[capture-token]");
    if (code !== 0 || size > 1024 * 1024)
      throw new Error(`cf capture command failed: ${output}`);
    return output;
  } finally {
    clearTimeout(timer);
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (
    (args.length !== 2 && args.length !== 3) ||
    !args.slice(0, 2).every(isAbsolute)
  )
    throw new Error(
      "usage: prepare-python-main.ts /absolute/prebuilt-project /absolute/new-destination [main|django|flask|fastapi|services|queues|workflows|runtime|durableobjects|durableobjects-retired]",
    );
  const fixture = args[2] ?? "main";
  if (
    fixture !== "main" &&
    fixture !== "django" &&
    fixture !== "flask" &&
    fixture !== "fastapi" &&
    fixture !== "services" &&
    fixture !== "queues" &&
    fixture !== "workflows" &&
    fixture !== "runtime" &&
    fixture !== "durableobjects" &&
    fixture !== "durableobjects-retired"
  )
    throw new Error("unknown Python capture fixture");
  const script = scripts[fixture];
  const uploadPath = `/client/v4/accounts/${account}/workers/scripts/${script}/versions`;
  const [input, destination] = args;
  if (!input || !destination) throw new Error("missing capture paths");
  const project = await realpath(input);
  const temporaryRoot = await realpath(join(repository, ".temp"));
  for (const path of [project, await realpath(dirname(destination))]) {
    const scope = relative(temporaryRoot, path);
    if (scope.startsWith("..") || isAbsolute(scope))
      throw new Error(
        "capture project and destination must be under repository .temp",
      );
  }
  // The official builder owns the supplied output; this tool never creates a bundle.
  const outputConfig = await readFile(
    join(project, ".cloudflare/output/v0/config.json"),
  );
  const cli = join(repository, "node_modules/.bin/cf");
  await mkdir(destination, { mode: 0o700 }); // Refuse overwrite; retain failure evidence.
  const secretsFile = join(destination, "capture-secrets.json");
  await writeFile(
    secretsFile,
    JSON.stringify({ TOKEN: "python-capture-fixture-token" }) + "\n",
    {
      flag: "wx",
      mode: 0o600,
    },
  );
  let captured:
    | {
        body: Buffer;
        description: Awaited<ReturnType<typeof describePythonUpload>>;
      }
    | undefined;
  const rejected: string[] = [];
  const assetDirectory = join(
    project,
    ".cloudflare/output/v0/workers/default/assets",
  );
  let assets: PythonAssetCapture | undefined;
  try {
    await realpath(assetDirectory);
    assets = new PythonAssetCapture(
      assetDirectory,
      destination,
      `/client/v4/accounts/${account}/workers/scripts/${script}/assets-upload-session`,
      `/client/v4/accounts/${account}/workers/assets/upload`,
      token,
    );
  } catch (error) {
    if (
      !(error instanceof Error) ||
      !("code" in error) ||
      error.code !== "ENOENT"
    )
      throw error;
  }
  const server = createServer((req, res) => {
    void (async () => {
      if (assets && (await assets.handle(req, res))) return;
      const path = new URL(req.url ?? "/", "http://127.0.0.1").pathname;
      const servicePath = `/client/v4/accounts/${account}/workers/services/${script}`;
      const settingsPath = `/client/v4/accounts/${account}/workers/scripts/${script}/settings`;
      const secretsPath = `/client/v4/accounts/${account}/workers/scripts/${script}/secrets`;
      const bucketPath = `/client/v4/accounts/${account}/r2/buckets/python-main-bucket`;
      const queuesPath = `/client/v4/accounts/${account}/queues`;
      const workflowPath = `/client/v4/accounts/${account}/workflows/python-workflows-flow`;
      const searchNamespacePrefix = `/client/v4/accounts/${account}/ai-search/namespaces/`;
      // cf resolves configured namespace names before upload. Keep that lookup
      // local, and continue rejecting provisioning mutations in this capture.
      if (
        fixture === "runtime" &&
        req.method === "GET" &&
        (path === `${searchNamespacePrefix}default` ||
          path === `${searchNamespacePrefix}python-runtime-isolated`) &&
        req.headers.authorization === `Bearer ${token}`
      ) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            success: true,
            errors: [],
            messages: [],
            result: { name: path.slice(searchNamespacePrefix.length) },
          }),
        );
        return;
      }
      // A workflow binding is resolved by cf before uploading the Version.
      // This developer-only catalog response never creates a remote workflow.
      if (
        fixture === "workflows" &&
        req.method === "GET" &&
        path === workflowPath &&
        req.headers.authorization === `Bearer ${token}`
      ) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            success: true,
            errors: [],
            messages: [],
            result: {
              id: "019c0000000070008000000000000005",
              name: "python-workflows-flow",
              script_name: script,
              class_name: "Flow",
              created_on: "2026-09-08T00:00:00.000Z",
              modified_on: "2026-09-08T00:00:00.000Z",
            },
          }),
        );
        return;
      }
      // Queue producers are resolved by cf before the Version upload. This is
      // an isolated catalog fixture; it does not create a remote Queue.
      if (
        fixture === "queues" &&
        req.method === "GET" &&
        path === queuesPath &&
        req.headers.authorization === `Bearer ${token}`
      ) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            success: true,
            errors: [],
            messages: [],
            result: [
              {
                queue_id: "019c0000000070008000000000000004",
                queue_name: "python-queues-events",
                created_on: "2026-09-08T00:00:00.000Z",
                modified_on: "2026-09-08T00:00:00.000Z",
                consumers: [],
                consumers_total_count: 0,
                producers: [],
                producers_total_count: 0,
                settings: {
                  delivery_delay: 0,
                  delivery_paused: true,
                  message_retention_period: 86400,
                },
              },
            ],
            result_info: {
              page: 1,
              per_page: 20,
              count: 1,
              total_count: 1,
              total_pages: 1,
            },
          }),
        );
        return;
      }
      // cf versions create checks the owning Script before publishing a Version.
      if (
        req.method === "GET" &&
        (path === servicePath ||
          path === settingsPath ||
          path === secretsPath ||
          path === bucketPath) &&
        req.headers.authorization === `Bearer ${token}`
      ) {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            success: true,
            errors: [],
            messages: [],
            result:
              path === bucketPath
                ? {
                    name: "python-main-bucket",
                    creation_date: "2026-09-08T00:00:00.000Z",
                    storage_class: "Standard",
                  }
                : path === secretsPath
                  ? []
                  : path === settingsPath
                    ? { bindings: [] }
                    : {
                        id: script,
                        default_environment: {
                          environment: "production",
                          script: {
                            id: script,
                            tag: "019c0000-0000-7000-8000-000000000003",
                            last_deployed_from: "open-compute",
                          },
                        },
                      },
          }),
        );
        return;
      }
      if (
        req.method !== "POST" ||
        path !== uploadPath ||
        req.headers.authorization !== `Bearer ${token}` ||
        captured
      ) {
        rejected.push(`${req.method} ${path}`);
        res.writeHead(404, { "content-type": "application/json" });
        res.end(
          JSON.stringify({
            success: false,
            errors: [{ code: 10007, message: "capture endpoint unavailable" }],
          }),
        );
        return;
      }
      const chunks: Buffer[] = [];
      let size = 0;
      for await (const chunk of req) {
        if (!Buffer.isBuffer(chunk)) throw new Error("invalid upload chunk");
        size += chunk.length;
        if (size > maxBody) throw new Error("SDK upload too large");
        chunks.push(chunk);
      }
      const body = Buffer.concat(chunks);
      const contentType = req.headers["content-type"] ?? "";
      const description = await describePythonUpload(
        contentType,
        body,
        fixture,
      ).catch(async (error: unknown) => {
        await writeFile(join(destination, "failed-upload.multipart"), body, {
          flag: "wx",
          mode: 0o600,
        });
        await writeFile(
          join(destination, "failed-upload.json"),
          JSON.stringify({
            contentType,
            error: error instanceof Error ? error.message : "invalid upload",
          }) + "\n",
          { flag: "wx", mode: 0o600 },
        );
        throw error;
      });
      captured = { body, description };
      assets?.assertComplete(description.metadata);
      res.writeHead(200, { "content-type": "application/json" });
      res.end(
        JSON.stringify({
          success: true,
          errors: [],
          messages: [],
          result: {
            id: "019c0000-0000-7000-8000-000000000002",
            metadata: { has_preview: false },
            startup_time_ms: 0,
          },
        }),
      );
    })().catch(() => {
      rejected.push("invalid multipart upload");
      res.writeHead(422);
      res.end();
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  try {
    const address = server.address();
    if (!address || typeof address === "string")
      throw new Error("missing capture address");
    const env = captureEnvironment(`http://127.0.0.1:${address.port}`);
    const versionOutput = await command(
      "node",
      [cli, "--version"],
      project,
      env,
    );
    const cf = /\bv?(1\.0\.0-beta\.21)(?:\s|$)/.exec(versionOutput)?.[1];
    if (!cf) throw new Error("cf version differs from fixture pin");
    const output = await command(
      "node",
      [
        cli,
        "workers",
        "versions",
        "create",
        "--prebuilt",
        ...(fixture === "durableobjects-retired" ? ["--mode", "retire"] : []),
        "--secrets-file",
        secretsFile,
      ],
      project,
      env,
    );
    await writeFile(join(destination, "cli.log"), output, {
      flag: "wx",
      mode: 0o600,
    });
    if (!captured || rejected.length)
      throw new Error(
        `capture incomplete; rejected requests: ${rejected.join(", ")}`,
      );
    await writeFile(join(destination, "upload.multipart"), captured.body, {
      flag: "wx",
    });
    await writeFile(
      join(destination, "manifest.json"),
      JSON.stringify(
        {
          cf,
          cliSha256: sha256(await readFile(cli)),
          buildOutputConfigSha256: sha256(outputConfig),
          producer: "cf workers versions create --prebuilt",
          ...captured.description,
        },
        null,
        2,
      ) + "\n",
      { flag: "wx" },
    );
    console.log(join(destination, "manifest.json"));
  } finally {
    server.closeAllConnections();
    if (server.listening)
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    if (rejected.length)
      await writeFile(
        join(destination, "rejected.json"),
        JSON.stringify(rejected) + "\n",
        {
          flag: "wx",
          mode: 0o600,
        },
      );
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1])
  await main();
