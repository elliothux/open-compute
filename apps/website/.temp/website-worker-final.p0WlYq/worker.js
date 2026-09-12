var __defProp = Object.defineProperty;
var __name = (target, value) => __defProp(target, "name", { value, configurable: true });

// functions/api/github-stars.ts
var repositoryUrl = "https://api.github.com/repos/elliothux/open-compute";
var successHeaders = {
  "cache-control": "public, max-age=300, s-maxage=300, stale-while-revalidate=3600",
  "content-type": "application/json; charset=utf-8"
};
var errorHeaders = {
  "cache-control": "no-store",
  "content-type": "application/json; charset=utf-8"
};
function isGitHubRepository(value) {
  if (typeof value !== "object" || value === null || !("stargazers_count" in value)) {
    return false;
  }
  const stars = value.stargazers_count;
  return Number.isInteger(stars) && typeof stars === "number" && stars >= 0;
}
__name(isGitHubRepository, "isGitHubRepository");
function jsonError(status, error) {
  return Response.json({ error }, { headers: errorHeaders, status });
}
__name(jsonError, "jsonError");
async function onRequestGet({
  env
}) {
  const token = env.VITE_GITHUB_PERSONAL_ACCESS_TOKEN?.trim();
  if (!token) return jsonError(503, "GitHub stars are unavailable");
  let response;
  try {
    response = await fetch(repositoryUrl, {
      headers: {
        accept: "application/vnd.github+json",
        authorization: `Bearer ${token}`,
        "user-agent": "open-compute.dev",
        "x-github-api-version": "2022-11-28"
      }
    });
  } catch {
    return jsonError(502, "GitHub stars are unavailable");
  }
  if (!response.ok) return jsonError(502, "GitHub stars are unavailable");
  let repository;
  try {
    repository = await response.json();
  } catch {
    return jsonError(502, "GitHub returned an invalid response");
  }
  if (!isGitHubRepository(repository)) {
    return jsonError(502, "GitHub returned an invalid response");
  }
  return Response.json(
    { stars: repository.stargazers_count },
    { headers: successHeaders }
  );
}
__name(onRequestGet, "onRequestGet");

// functions/api/releases.ts
var releasesUrl = "https://api.github.com/repos/elliothux/open-compute/releases?per_page=10";
var successHeaders2 = {
  "cache-control": "public, max-age=1800, s-maxage=1800, stale-while-revalidate=86400",
  "content-type": "application/json; charset=utf-8"
};
var errorHeaders2 = {
  "cache-control": "no-store",
  "content-type": "application/json; charset=utf-8"
};
function isTrustedReleaseUrl(value) {
  try {
    const url = new URL(value);
    return url.protocol === "https:" && url.hostname === "github.com" && url.pathname.startsWith("/elliothux/open-compute/releases/");
  } catch {
    return false;
  }
}
__name(isTrustedReleaseUrl, "isTrustedReleaseUrl");
function isGitHubRelease(value) {
  if (typeof value !== "object" || value === null) return false;
  return "body" in value && (typeof value.body === "string" || value.body === null) && "draft" in value && typeof value.draft === "boolean" && "html_url" in value && typeof value.html_url === "string" && isTrustedReleaseUrl(value.html_url) && "name" in value && (typeof value.name === "string" || value.name === null) && "prerelease" in value && typeof value.prerelease === "boolean" && "published_at" in value && (typeof value.published_at === "string" || value.published_at === null) && "tag_name" in value && typeof value.tag_name === "string" && value.tag_name.length > 0;
}
__name(isGitHubRelease, "isGitHubRelease");
function cleanMarkdownLine(line) {
  return line.replace(/^\s*(?:[-*+]\s+|#+\s*)/, "").replace(/!\[[^\]]*\]\([^)]*\)/g, "").replace(/\[([^\]]+)]\([^)]+\)/g, "$1").replace(/[`*_~]/g, "").replace(/<[^>]+>/g, "").replace(/\s+/g, " ").trim();
}
__name(cleanMarkdownLine, "cleanMarkdownLine");
function summarizeRelease(release) {
  const bodyLine = release.body?.split(/\r?\n\s*\r?\n/).map((paragraph) => paragraph.trim()).filter((paragraph) => paragraph.length > 0 && !paragraph.startsWith("#")).map(cleanMarkdownLine).find(
    (paragraph) => paragraph.length > 0 && !/^(what'?s changed|new contributors|full changelog)$/i.test(paragraph)
  );
  const summary = bodyLine ?? (release.name && release.name !== release.tag_name ? cleanMarkdownLine(release.name) : "Release notes on GitHub.");
  return summary.length > 220 ? `${summary.slice(0, 217).trimEnd()}...` : summary;
}
__name(summarizeRelease, "summarizeRelease");
function jsonError2(status, error) {
  return Response.json({ error }, { headers: errorHeaders2, status });
}
__name(jsonError2, "jsonError");
async function onRequestGet2({
  env
}) {
  const headers = {
    accept: "application/vnd.github+json",
    "user-agent": "open-compute.dev",
    "x-github-api-version": "2022-11-28"
  };
  const token = env.VITE_GITHUB_PERSONAL_ACCESS_TOKEN?.trim();
  if (token) headers.authorization = `Bearer ${token}`;
  let response;
  try {
    response = await fetch(releasesUrl, { headers });
  } catch {
    return jsonError2(502, "GitHub releases are unavailable");
  }
  if (!response.ok) return jsonError2(502, "GitHub releases are unavailable");
  let payload;
  try {
    payload = await response.json();
  } catch {
    return jsonError2(502, "GitHub returned an invalid response");
  }
  if (!Array.isArray(payload) || !payload.every(isGitHubRelease)) {
    return jsonError2(502, "GitHub returned an invalid response");
  }
  const releases = payload.filter(
    (release) => !release.draft && !release.prerelease && release.published_at !== null
  ).slice(0, 3).map((release) => ({
    publishedAt: release.published_at,
    summary: summarizeRelease(release),
    tagName: release.tag_name.replace(/^v(?=\d)/i, ""),
    url: release.html_url
  }));
  return Response.json({ releases }, { headers: successHeaders2 });
}
__name(onRequestGet2, "onRequestGet");

// src/worker.ts
var apiRoutes = {
  "/api/github-stars": onRequestGet,
  "/api/releases": onRequestGet2
};
function methodNotAllowed() {
  return Response.json(
    { error: "Method not allowed" },
    {
      headers: {
        allow: "GET, HEAD",
        "cache-control": "no-store"
      },
      status: 405
    }
  );
}
__name(methodNotAllowed, "methodNotAllowed");
var worker_default = {
  async fetch(request, env) {
    const pathname = new URL(request.url).pathname.replace(/\/+$/, "");
    const handler = apiRoutes[pathname];
    if (!handler) return env.ASSETS.fetch(request);
    if (request.method !== "GET" && request.method !== "HEAD") {
      return methodNotAllowed();
    }
    const response = await handler({ env });
    if (request.method === "HEAD") {
      return new Response(null, {
        headers: response.headers,
        status: response.status,
        statusText: response.statusText
      });
    }
    return response;
  }
};
export {
  worker_default as default
};
//# sourceMappingURL=worker.js.map
