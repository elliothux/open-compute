import type { RuntimeAssets, RuntimeAssetManifest, RuntimeAssetRouting } from "../loader/protocol.js";

type Selection = "entry" | "redirect" | "missing";

function find(manifest: RuntimeAssetManifest, path: string): boolean {
  let low = 0;
  let high = manifest.entries.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    const candidate = manifest.entries[middle]!.path;
    if (candidate === path) return true;
    if (candidate < path) low = middle + 1;
    else high = middle;
  }
  return false;
}

function trailing(path: string): string {
  return path === "/" ? "/" : `${path}/`;
}

function selection(manifest: RuntimeAssetManifest, routing: RuntimeAssetRouting, path: string): Selection {
  if (routing.htmlHandling === "none") return find(manifest, path) ? "entry" : "missing";
  if (routing.htmlHandling === "auto-trailing-slash") {
    const suffix = path.endsWith("/index.html") ? "/index.html"
      : path.endsWith("/index") ? "/index"
      : path.endsWith(".html") ? ".html" : undefined;
    if (suffix) {
      const raw = path.slice(0, -suffix.length);
      const alias = raw || "/";
      const file = `${alias}.html`;
      const index = alias === "/" ? "/index.html" : `${alias}/index.html`;
      if (find(manifest, file) || find(manifest, index)) return "redirect";
    }
  }
  const trimmed = path.replace(/\/+$/, "") || "/";
  const withoutHtml = trimmed.endsWith(".html") ? trimmed.slice(0, -5) : trimmed;
  const rawStem = withoutHtml.endsWith("/index") ? withoutHtml.slice(0, -6) : withoutHtml;
  const stem = rawStem || "/";
  const file = `${stem}.html`;
  const index = stem === "/" ? "/index.html" : `${stem}/index.html`;
  const fileFound = find(manifest, file);
  const indexFound = find(manifest, index);
  let found = false;
  let canonical = path;
  switch (routing.htmlHandling) {
    case "auto-trailing-slash":
      found = fileFound || indexFound || find(manifest, path);
      canonical = fileFound ? stem : indexFound ? trailing(stem) : path;
      break;
    case "force-trailing-slash":
      found = fileFound || indexFound || find(manifest, path);
      canonical = fileFound || indexFound ? trailing(stem) : path;
      break;
    case "drop-trailing-slash":
      found = fileFound || indexFound || find(manifest, path);
      canonical = fileFound || indexFound ? stem : path;
      break;
  }
  if (!found) return "missing";
  return canonical === path ? "entry" : "redirect";
}

function match(pattern: string, value: string): boolean {
  let expression = "";
  for (let index = 0; index < pattern.length;) {
    const character = pattern[index]!;
    if (character === "*") {
      expression += ".*";
      index += 1;
    } else if (character === ":") {
      let end = index + 1;
      while (end < pattern.length && /[A-Za-z0-9_]/.test(pattern[end]!)) end += 1;
      expression += pattern.slice(0, index).includes("/") ? "[^/]+" : "[^.]+";
      index = end;
    } else {
      expression += character.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
      index += 1;
    }
  }
  return new RegExp(`^${expression}$`).test(value);
}

function ruleMatches(pattern: string, request: Request): boolean {
  const url = new URL(request.url);
  return match(pattern.startsWith("https://") ? pattern.slice(8) : pattern,
    pattern.startsWith("https://") ? `${url.hostname}${url.pathname}` : url.pathname);
}

function canFetch(assets: RuntimeAssets, request: Request): boolean {
  if (request.method !== "GET" && request.method !== "HEAD") return false;
  if (assets.routing.redirects.some((rule) => ruleMatches(rule.from, request))) return true;
  return selection(assets.manifest, assets.routing, new URL(request.url).pathname) !== "missing";
}

/** Choose the trusted asset handler or tenant Worker without invoking either path speculatively. */
export function routeDefaultHttp(snapshot: {
  assets?: RuntimeAssets;
  contentKind: "worker" | "assets_only";
  compatibilityDate: string;
  compatibilityFlags: string[];
}, request: Request): "asset" | "worker" {
  const assets = snapshot.assets;
  if (!assets) return "worker";
  const hasWorker = snapshot.contentKind === "worker";
  const mode = assets.routing.runWorkerFirst;
  if (mode === true && hasWorker) return "worker";
  if (Array.isArray(mode) && hasWorker) {
    const path = new URL(request.url).pathname;
    if (mode.filter((rule) => rule.startsWith("!")).some((rule) => match(rule.slice(1), path))) return "asset";
    if (mode.filter((rule) => !rule.startsWith("!")).some((rule) => match(rule, path))) return "worker";
    return "asset";
  }
  if (canFetch(assets, request) || !hasWorker) return "asset";
  const navigation = snapshot.compatibilityFlags.includes("assets_navigation_prefers_asset_serving")
    || (snapshot.compatibilityDate >= "2025-04-01"
      && !snapshot.compatibilityFlags.includes("assets_navigation_has_no_effect"));
  return navigation && request.headers.get("sec-fetch-mode") === "navigate"
    && assets.routing.notFoundHandling !== "none" ? "asset" : "worker";
}
