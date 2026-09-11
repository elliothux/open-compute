interface Environment {
  VITE_GITHUB_PERSONAL_ACCESS_TOKEN?: string;
}

interface PagesFunctionContext {
  env: Environment;
}

interface GitHubRelease {
  body: string | null;
  draft: boolean;
  html_url: string;
  name: string | null;
  prerelease: boolean;
  published_at: string | null;
  tag_name: string;
}

const releasesUrl =
  "https://api.github.com/repos/elliothux/open-compute/releases?per_page=10";

const successHeaders = {
  "cache-control":
    "public, max-age=1800, s-maxage=1800, stale-while-revalidate=86400",
  "content-type": "application/json; charset=utf-8",
} as const;

const errorHeaders = {
  "cache-control": "no-store",
  "content-type": "application/json; charset=utf-8",
} as const;

function isTrustedReleaseUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === "https:" &&
      url.hostname === "github.com" &&
      url.pathname.startsWith("/elliothux/open-compute/releases/")
    );
  } catch {
    return false;
  }
}

function isGitHubRelease(value: unknown): value is GitHubRelease {
  if (typeof value !== "object" || value === null) return false;

  return (
    "body" in value &&
    (typeof value.body === "string" || value.body === null) &&
    "draft" in value &&
    typeof value.draft === "boolean" &&
    "html_url" in value &&
    typeof value.html_url === "string" &&
    isTrustedReleaseUrl(value.html_url) &&
    "name" in value &&
    (typeof value.name === "string" || value.name === null) &&
    "prerelease" in value &&
    typeof value.prerelease === "boolean" &&
    "published_at" in value &&
    (typeof value.published_at === "string" || value.published_at === null) &&
    "tag_name" in value &&
    typeof value.tag_name === "string" &&
    value.tag_name.length > 0
  );
}

function cleanMarkdownLine(line: string): string {
  return line
    .replace(/^\s*(?:[-*+]\s+|#+\s*)/, "")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, "")
    .replace(/\[([^\]]+)]\([^)]+\)/g, "$1")
    .replace(/[`*_~]/g, "")
    .replace(/<[^>]+>/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function summarizeRelease(release: GitHubRelease): string {
  const bodyLine = release.body
    ?.split(/\r?\n\s*\r?\n/)
    .map((paragraph) => paragraph.trim())
    .filter((paragraph) => paragraph.length > 0 && !paragraph.startsWith("#"))
    .map(cleanMarkdownLine)
    .find(
      (paragraph) =>
        paragraph.length > 0 &&
        !/^(what'?s changed|new contributors|full changelog)$/i.test(paragraph),
    );
  const summary =
    bodyLine ??
    (release.name && release.name !== release.tag_name
      ? cleanMarkdownLine(release.name)
      : "Release notes on GitHub.");

  return summary.length > 220
    ? `${summary.slice(0, 217).trimEnd()}...`
    : summary;
}

function jsonError(status: number, error: string): Response {
  return Response.json({ error }, { headers: errorHeaders, status });
}

export async function onRequestGet({
  env,
}: PagesFunctionContext): Promise<Response> {
  const headers: Record<string, string> = {
    accept: "application/vnd.github+json",
    "user-agent": "open-compute.dev",
    "x-github-api-version": "2022-11-28",
  };
  const token = env.VITE_GITHUB_PERSONAL_ACCESS_TOKEN?.trim();
  if (token) headers.authorization = `Bearer ${token}`;

  let response: Response;
  try {
    response = await fetch(releasesUrl, { headers });
  } catch {
    return jsonError(502, "GitHub releases are unavailable");
  }

  if (!response.ok) return jsonError(502, "GitHub releases are unavailable");

  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    return jsonError(502, "GitHub returned an invalid response");
  }

  if (!Array.isArray(payload) || !payload.every(isGitHubRelease)) {
    return jsonError(502, "GitHub returned an invalid response");
  }

  const releases = payload
    .filter(
      (release) =>
        !release.draft && !release.prerelease && release.published_at !== null,
    )
    .slice(0, 3)
    .map((release) => ({
      publishedAt: release.published_at,
      summary: summarizeRelease(release),
      tagName: release.tag_name.replace(/^v(?=\d)/i, ""),
      url: release.html_url,
    }));

  return Response.json({ releases }, { headers: successHeaders });
}
