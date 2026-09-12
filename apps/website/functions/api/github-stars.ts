interface Environment {
  VITE_GITHUB_PERSONAL_ACCESS_TOKEN?: string;
}

interface PagesFunctionContext {
  env: Environment;
}

interface GitHubRepository {
  stargazers_count: number;
}

const repositoryUrl = "https://api.github.com/repos/elliothux/open-compute";

const successHeaders = {
  "cache-control":
    "public, max-age=300, s-maxage=300, stale-while-revalidate=3600",
  "content-type": "application/json; charset=utf-8",
} as const;

const errorHeaders = {
  "cache-control": "no-store",
  "content-type": "application/json; charset=utf-8",
} as const;

function isGitHubRepository(value: unknown): value is GitHubRepository {
  if (
    typeof value !== "object" ||
    value === null ||
    !("stargazers_count" in value)
  ) {
    return false;
  }

  const stars = value.stargazers_count;
  return Number.isInteger(stars) && typeof stars === "number" && stars >= 0;
}

function jsonError(status: number, error: string): Response {
  return Response.json({ error }, { headers: errorHeaders, status });
}

export async function onRequestGet({
  env,
}: PagesFunctionContext): Promise<Response> {
  const token = env.VITE_GITHUB_PERSONAL_ACCESS_TOKEN?.trim();
  if (!token) return jsonError(503, "GitHub stars are unavailable");

  let response: Response;
  try {
    response = await fetch(repositoryUrl, {
      headers: {
        accept: "application/vnd.github+json",
        authorization: `Bearer ${token}`,
        "user-agent": "open-compute.dev",
        "x-github-api-version": "2022-11-28",
      },
    });
  } catch {
    return jsonError(502, "GitHub stars are unavailable");
  }

  if (!response.ok) return jsonError(502, "GitHub stars are unavailable");

  let repository: unknown;
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
    { headers: successHeaders },
  );
}
