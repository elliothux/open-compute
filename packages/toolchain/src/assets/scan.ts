import { createHash } from "node:crypto";
import { constants } from "node:fs";
import { open, opendir, realpath } from "node:fs/promises";
import { isAbsolute, relative, resolve, sep } from "node:path";
import type { AssetsProject, AssetManifestEntry, AssetObjectSource, ScannedAssets } from "./types.ts";
import { parseHeaders, parseRedirects } from "./rules.ts";

const MAX_FILE_BYTES = 25 * 1024 * 1024;
const MAX_FILES = 20_000;
const MAX_TOTAL_BYTES = 512 * 1024 * 1024;
const MAX_MANIFEST_BYTES = 16 * 1024 * 1024;
const CONTROL_FILES = new Set([".assetsignore", "_headers", "_redirects"]);
const FORBIDDEN = /(^|\/)(?:\.git(?:\/|$)|\.env(?:\.|$)|credentials?(?:\.|$)|secrets?(?:\.|$))/i;

const MIME = new Map<string, string>([
  ["css", "text/css; charset=utf-8"], ["csv", "text/csv; charset=utf-8"],
  ["gif", "image/gif"], ["htm", "text/html; charset=utf-8"],
  ["html", "text/html; charset=utf-8"], ["ico", "image/x-icon"],
  ["jpeg", "image/jpeg"], ["jpg", "image/jpeg"], ["js", "text/javascript; charset=utf-8"],
  ["json", "application/json; charset=utf-8"], ["map", "application/json; charset=utf-8"],
  ["mjs", "text/javascript; charset=utf-8"], ["avif", "image/avif"],
  ["png", "image/png"], ["svg", "image/svg+xml"], ["txt", "text/plain; charset=utf-8"],
  ["wasm", "application/wasm"], ["webmanifest", "application/manifest+json; charset=utf-8"],
  ["webp", "image/webp"], ["woff", "font/woff"], ["woff2", "font/woff2"],
  ["xml", "application/xml; charset=utf-8"], ["pdf", "application/pdf"],
]);

function within(root: string, value: string): boolean {
  const child = relative(root, value);
  return child !== ".." && !child.startsWith(`..${sep}`) && !isAbsolute(child);
}

function ignored(path: string, rules: readonly string[]): boolean {
  let value = false;
  for (const rule of rules) {
    const negative = rule.startsWith("!");
    const pattern = negative ? rule.slice(1) : rule;
    const expression = new RegExp(`^${pattern.split("*").map(part => part.replace(/[\\^$+?.()|[\]{}]/g, "\\$&")).join(".*")}$`);
    if (expression.test(path) || expression.test(path.split("/").at(-1) ?? "")) value = !negative;
  }
  return value;
}

function urlPath(path: string): string {
  return `/${path.split("/").map(segment => encodeURIComponent(segment)).join("/")}`;
}

function contentType(path: string): string {
  const filename = path.split("/").at(-1) ?? "";
  const extension = filename.includes(".") ? filename.split(".").at(-1)!.toLowerCase() : "";
  return MIME.get(extension) ?? "application/octet-stream";
}

async function openPinned(path: string, directory: boolean): Promise<Awaited<ReturnType<typeof open>>> {
  const flags = constants.O_RDONLY | constants.O_NOFOLLOW | (directory ? constants.O_DIRECTORY : 0);
  return open(path, flags);
}

async function readBounded(file: Awaited<ReturnType<typeof open>>, maximum: number, label: string): Promise<Uint8Array> {
  const info = await file.stat();
  if (!info.isFile() || info.size > maximum) throw new Error(`${label} is not a bounded regular file`);
  const bytes = Buffer.alloc(info.size);
  let offset = 0;
  while (offset < bytes.length) {
    const { bytesRead } = await file.read(bytes, offset, bytes.length - offset, offset);
    if (!bytesRead) break;
    offset += bytesRead;
  }
  const after = await file.stat();
  if (offset !== bytes.length || after.size !== info.size || after.mtimeMs !== info.mtimeMs) {
    throw new Error(`${label} changed during the asset scan`);
  }
  return bytes;
}

async function readControl(root: string, name: string): Promise<string | undefined> {
  try {
    const filename = resolve(root, name);
    const file = await openPinned(filename, false);
    try {
      const resolved = await realpath(filename);
      if (!within(root, resolved)) throw new Error(`${name} escapes the asset directory`);
      return new TextDecoder("utf-8", { fatal: true }).decode(await readBounded(file, 256 * 1024, name));
    } finally { await file.close(); }
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") return undefined;
    throw error;
  }
}

/** Scan one explicit output root through pinned directory/file descriptors. */
export async function scanAssets(projectRoot: string, config: AssetsProject): Promise<ScannedAssets> {
  const project = await realpath(projectRoot);
  const directory = await realpath(resolve(project, config.directory));
  if (!within(project, directory) || directory === project) throw new Error("assets.directory must be a dedicated directory inside the project");
  const ignoreContent = await readControl(directory, ".assetsignore");
  const ignoreRules = (ignoreContent ?? "").replaceAll("\r\n", "\n").split("\n")
    .map(line => line.trim()).filter(line => line && !line.startsWith("#"));
  const entries: AssetManifestEntry[] = [];
  const objects = new Map<string, AssetObjectSource>();
  let total = 0;

  const walk = async (path: string, logical: string): Promise<void> => {
    const handle = await openPinned(path, true);
    try {
      const actual = await realpath(path);
      if (!within(directory, actual)) throw new Error("asset directory traversal escaped its root");
      const listing = await opendir(path);
      try {
        for await (const item of listing) {
          const childLogical = logical ? `${logical}/${item.name}` : item.name;
          if (ignored(childLogical, ignoreRules)) continue;
          if (FORBIDDEN.test(childLogical)) throw new Error(`asset output contains forbidden path: ${childLogical}`);
          const child = resolve(path, item.name);
          if (item.isSymbolicLink()) throw new Error(`asset output contains a symbolic link: ${childLogical}`);
          if (item.isDirectory()) { await walk(child, childLogical); continue; }
          if (!item.isFile()) throw new Error(`asset output contains a special file: ${childLogical}`);
          if (CONTROL_FILES.has(childLogical)) continue;
          if (!config.publishSourceMaps && childLogical.endsWith(".map")) continue;
          const file = await openPinned(child, false);
          try {
            const bytes = await readBounded(file, MAX_FILE_BYTES, childLogical);
            total += bytes.byteLength;
            if (total > MAX_TOTAL_BYTES) throw new Error("asset output exceeds 512 MiB");
            if (entries.length >= MAX_FILES) throw new Error("asset output exceeds 20000 files");
            const sha256 = createHash("sha256").update(bytes).digest("hex");
            const path = urlPath(childLogical);
            entries.push({ path, sha256, size: bytes.byteLength, contentType: contentType(childLogical) });
            if (!objects.has(sha256)) {
              objects.set(sha256, {
                filename: resolve(directory, ...childLogical.split("/")),
                sha256,
                size: bytes.byteLength,
              });
            }
            else if (objects.get(sha256)!.size !== bytes.byteLength) throw new Error("asset digest declares conflicting lengths");
          } finally { await file.close(); }
        }
      } finally {
        try { await listing.close(); } catch { /* `for await` closes an exhausted directory. */ }
      }
    } finally { await handle.close(); }
  };
  await walk(directory, "");
  entries.sort((left, right) => Buffer.compare(Buffer.from(left.path), Buffer.from(right.path)));
  if (!entries.length) throw new Error("asset output contains no publishable files");
  if (entries.some((entry, index) => index > 0 && entries[index - 1]!.path === entry.path)) {
    throw new Error("asset output contains duplicate canonical URL paths");
  }
  const manifest = { schemaVersion: 1 as const, entries };
  if (Buffer.byteLength(JSON.stringify(manifest)) > MAX_MANIFEST_BYTES) throw new Error("asset manifest exceeds 16 MiB");
  const headers = parseHeaders((await readControl(directory, "_headers")) ?? "");
  const redirects = parseRedirects((await readControl(directory, "_redirects")) ?? "");
  return {
    manifest,
    routing: {
      schemaVersion: 1,
      ...(config.binding === undefined ? {} : { binding: config.binding }),
      runWorkerFirst: config.runWorkerFirst,
      htmlHandling: config.htmlHandling,
      notFoundHandling: config.notFoundHandling,
      headers,
      redirects,
    },
    objects,
  };
}

/** Re-read one bounded source and prove the bytes still match the frozen manifest. */
export async function readAssetObject(source: AssetObjectSource): Promise<Uint8Array> {
  const file = await openPinned(source.filename, false);
  try {
    const bytes = await readBounded(file, MAX_FILE_BYTES, "asset object");
    const digest = createHash("sha256").update(bytes).digest("hex");
    if (bytes.byteLength !== source.size || digest !== source.sha256) throw new Error("asset changed after manifest generation");
    return bytes;
  } finally { await file.close(); }
}
