// Explicit preparation of an installed browser; ocd never invokes this tool.
import { spawn } from "node:child_process";
import { constants } from "node:fs";
import {
  cp,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { dirname, isAbsolute, join } from "node:path";
import { gzipSync } from "node:zlib";
import { absoluteDestination, repository, sha256 } from "./workerd-archive.ts";

type Asset = { path: string; sha256: string; mediaType: string; data: string };
const maximum = 8 * 1024 * 1024;
const media: Readonly<Record<string, string>> = {
  html: "text/html; charset=utf-8",
  js: "text/javascript; charset=utf-8",
  css: "text/css; charset=utf-8",
  json: "application/json",
  svg: "image/svg+xml",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  webp: "image/webp",
  gif: "image/gif",
  woff: "font/woff",
  woff2: "font/woff2",
  wasm: "application/wasm",
};
function record(value: unknown): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value))
    throw new Error("invalid native browser response");
  return value as Record<string, unknown>;
}
function string(value: unknown): string {
  if (typeof value !== "string" || !value)
    throw new Error("invalid native browser response field");
  return value;
}
const delay = (milliseconds: number) =>
  new Promise<void>((resolve) => setTimeout(resolve, milliseconds));

let source: string | undefined;
let destination: string | undefined;
for (let index = 2; index < process.argv.length; index++) {
  const option = process.argv[index];
  const value = process.argv[++index];
  if (!value || value.startsWith("--"))
    throw new Error("--source and --dest require explicit absolute paths");
  if (option === "--source" && source === undefined) source = value;
  else if (option === "--dest" && destination === undefined)
    destination = value;
  else throw new Error("unexpected browser preparation argument");
}
if (
  !source ||
  !destination ||
  !isAbsolute(source) ||
  /[\r\n]/.test(source) ||
  source.split("/").includes("..") ||
  (await realpath(source)) !== source
)
  throw new Error(
    "--source and --dest are required absolute paths without symlinks",
  );
const input = await lstat(source);
if (
  !input.isFile() ||
  input.mode & 0o022 ||
  !(input.mode & 0o100) ||
  input.size > 1024 * 1024 * 1024
)
  throw new Error("unsafe browser executable");
const binary = await readFile(source);
const binarySha256 = sha256(binary);
const candidates = [
  ...new Set(
    [
      ...binary
        .toString("latin1")
        .matchAll(
          /(?<=\0)(?:\/devtools\/)?(?:[A-Za-z0-9_.-]+\/)*[A-Za-z0-9_.-]+\.(?:js|html|css|svg|woff2?|wasm|png|json|jpe?g|gif|webp)(?=\0)/g,
        ),
    ].map((match) => match[0].replace(/^\/devtools\//, "")),
  ),
]
  .filter(
    (path) =>
      path.length <= 1024 &&
      path.split("/").every((part) => part !== "." && part !== ".."),
  )
  .sort();
if (!candidates.includes("inspector.html") || candidates.length > 4096)
  throw new Error("browser native frontend resource table unavailable");
const target = await absoluteDestination(destination);
const purpose = join(repository, ".temp/browser-preparation");
await mkdir(purpose, { recursive: true, mode: 0o700 });
const scratch = await mkdtemp(join(purpose, "run-"));
await mkdir(join(scratch, "tmp"), { mode: 0o700 });
let child: ReturnType<typeof spawn> | undefined;
let finished = false;
let completed: Promise<void> | undefined;
let socket: WebSocket | undefined;
let stderr = "";
let successful = false;
let targetCreated = false;
let preparedExecutable = "";
let preparedAssets = 0;
try {
  targetCreated = true;
  await cp(dirname(source), target, {
    recursive: true,
    errorOnExist: true,
    force: false,
    mode: constants.COPYFILE_FICLONE,
    filter: async (path) => {
      const entry = await lstat(path);
      if ((!entry.isFile() && !entry.isDirectory()) || entry.mode & 0o022)
        throw new Error("unsafe browser installation entry");
      return true;
    },
  });
  const executable = join(target, source.split("/").at(-1) ?? "");
  if (sha256(await readFile(executable)) !== binarySha256)
    throw new Error("browser executable changed during preparation");
  child = spawn(
    executable,
    [
      "--remote-debugging-address=127.0.0.1",
      "--remote-debugging-port=0",
      "--disable-gpu",
      "--disable-background-networking",
      "--disable-extensions",
      "--disable-component-update",
      "--no-first-run",
      "--no-default-browser-check",
      `--user-data-dir=${join(scratch, "profile")}`,
      "about:blank",
    ],
    {
      cwd: scratch,
      env: { HOME: scratch, TMPDIR: join(scratch, "tmp") },
      stdio: ["ignore", "ignore", "pipe"],
    },
  );
  completed = new Promise<void>((resolve) => {
    const end = () => {
      finished = true;
      resolve();
    };
    child?.once("exit", end);
    child?.once("error", end);
  });
  child.stderr?.on("data", (bytes: Buffer) => {
    stderr = (stderr + bytes.toString()).slice(-16384);
  });
  const deadline = performance.now() + 60_000;
  let port: number | undefined;
  while (port === undefined) {
    if (finished || performance.now() > deadline)
      throw new Error("native browser readiness failed");
    try {
      const text = await readFile(
        join(scratch, "profile/DevToolsActivePort"),
        "utf8",
      );
      const candidate = Number(text.split("\n")[0]);
      if (!Number.isInteger(candidate) || candidate < 1 || candidate > 65535)
        throw new Error("invalid native debugger port");
      port = candidate;
    } catch (error) {
      if (!(
        error instanceof Error &&
        "code" in error &&
        error.code === "ENOENT"
      ))
        throw error;
      await delay(25);
    }
  }
  const origin = `http://127.0.0.1:${port}`;
  const bytes = async (path: string): Promise<Buffer> => {
    if (performance.now() > deadline)
      throw new Error("browser preparation deadline exceeded");
    const response = await fetch(`${origin}/${path}`, {
      redirect: "error",
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok || !response.body)
      throw new Error("native resource request failed");
    const chunks: Uint8Array[] = [];
    let length = 0;
    for await (const chunk of response.body) {
      length += chunk.byteLength;
      if (length > maximum)
        throw new Error("native resource exceeds preparation limit");
      chunks.push(chunk);
    }
    return Buffer.concat(chunks);
  };
  const native = record(
    JSON.parse((await bytes("json/version")).toString()) as unknown,
  );
  const product = string(native.Browser);
  if (!/^HeadlessChrome\/153\.[0-9]+\.[0-9]+\.[0-9]+$/.test(product))
    throw new Error("unsupported installed browser version");
  const revision = string(native["WebKit-Version"]).match(
    /\(@([a-f0-9]{40})\)$/,
  )?.[1];
  if (!revision) throw new Error("native browser source identity unavailable");
  const endpoint = new URL(string(native.webSocketDebuggerUrl));
  if (
    endpoint.protocol !== "ws:" ||
    endpoint.hostname !== "127.0.0.1" ||
    Number(endpoint.port) !== port ||
    !endpoint.pathname.startsWith("/devtools/browser/") ||
    endpoint.username ||
    endpoint.password ||
    endpoint.hash ||
    endpoint.search
  )
    throw new Error("invalid native browser control endpoint");
  socket = new WebSocket(endpoint);
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(
      () => reject(new Error("native browser control connection deadline")),
      5000,
    );
    socket?.addEventListener(
      "open",
      () => {
        clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
    socket?.addEventListener(
      "error",
      () => {
        clearTimeout(timer);
        reject(new Error("native browser control connection failed"));
      },
      { once: true },
    );
  });
  const assets: Asset[] = [];
  let total = 0;
  for (const path of candidates) {
    const data = await bytes(`devtools/${path}`);
    // Chromium returns 200/empty for unknown resource names.
    if (!data.length) continue;
    total += data.length;
    if (total > 24 * 1024 * 1024)
      throw new Error("native frontend exceeds preparation budget");
    assets.push({
      path,
      sha256: sha256(data),
      mediaType:
        media[path.split(".").at(-1) ?? ""] ?? "application/octet-stream",
      data: data.toString("base64"),
    });
  }
  const license = await readFile(join(target, "LICENSE.headless_shell"));
  if (!license.length || license.length > maximum)
    throw new Error("native browser license unavailable");
  assets.push({
    path: "LICENSE.headless_shell",
    sha256: sha256(license),
    mediaType: "text/plain; charset=utf-8",
    data: license.toString("base64"),
  });
  for (const name of ["inspector.html", "entrypoints/inspector/inspector.js"])
    if (!assets.some((asset) => asset.path === name))
      throw new Error("required native frontend entrypoint unavailable");
  const protocol: unknown = JSON.parse(
    (await bytes("json/protocol")).toString(),
  );
  const packed = Buffer.from(
    JSON.stringify({
      binarySha256,
      version: product.replace("HeadlessChrome/", "Google Chrome for Testing "),
      revision: `@${revision}`,
      protocol,
      assets: assets.sort((a, b) =>
        a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
      ),
    }),
  );
  if (packed.length > 32 * 1024 * 1024)
    throw new Error("native frontend manifest exceeds limit");
  await writeFile(
    join(target, "browser-devtools.json.gz"),
    gzipSync(packed, { level: 9 }),
    { flag: "wx", mode: 0o400 },
  );
  successful = true;
  preparedExecutable = executable;
  preparedAssets = assets.length;
} finally {
  if (socket?.readyState === WebSocket.OPEN)
    socket.send(JSON.stringify({ id: 1, method: "Browser.close" }));
  if (completed) {
    await Promise.race([completed, delay(1000)]);
    if (!finished) {
      child?.kill("SIGTERM");
      await Promise.race([completed, delay(1000)]);
    }
    if (!finished) {
      child?.kill("SIGKILL");
      await Promise.race([completed, delay(5000)]);
    }
    if (!finished) {
      successful = false;
      console.error(
        "Native browser did not exit; preparation evidence retained",
      );
    }
  }
  socket?.close();
  await writeFile(join(scratch, "stderr.log"), stderr, { mode: 0o600 });
  if (successful) await rm(scratch, { recursive: true });
  else if (targetCreated)
    console.error(`Incomplete prepared installation retained: ${target}`);
}
if (!finished) throw new Error("native browser did not exit");
console.log(`OPEN_COMPUTE_TEST_BROWSER=${preparedExecutable}`);
console.log(`OPEN_COMPUTE_BROWSER_FRONTEND_ASSETS=${preparedAssets}`);
