import { onRequestGet as getGitHubStars } from "../functions/api/github-stars";
import { onRequestGet as getGitHubReleases } from "../functions/api/releases";

interface AssetsBinding {
  fetch(request: Request): Promise<Response>;
}

interface WorkerEnvironment {
  ASSETS: AssetsBinding;
  VITE_GITHUB_PERSONAL_ACCESS_TOKEN?: string;
}

type ApiHandler = (context: { env: WorkerEnvironment }) => Promise<Response>;

const apiRoutes: Readonly<Record<string, ApiHandler>> = {
  "/api/github-stars": getGitHubStars,
  "/api/releases": getGitHubReleases,
};

function methodNotAllowed(): Response {
  return Response.json(
    { error: "Method not allowed" },
    {
      headers: {
        allow: "GET, HEAD",
        "cache-control": "no-store",
      },
      status: 405,
    },
  );
}

export default {
  async fetch(request: Request, env: WorkerEnvironment): Promise<Response> {
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
        statusText: response.statusText,
      });
    }
    return response;
  },
};
