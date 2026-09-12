import { fileURLToPath } from "node:url";
import type { APIRoute } from "astro";
import { loadEnv } from "vite";
import { onRequestGet as getGitHubStars } from "../functions/api/github-stars";
import { onRequestGet as getGitHubReleases } from "../functions/api/releases";

const projectDirectory = fileURLToPath(new URL("..", import.meta.url));
const variables = loadEnv("development", projectDirectory, "");
const environment = variables.VITE_GITHUB_PERSONAL_ACCESS_TOKEN
  ? {
      VITE_GITHUB_PERSONAL_ACCESS_TOKEN:
        variables.VITE_GITHUB_PERSONAL_ACCESS_TOKEN,
    }
  : {};

export const GET: APIRoute = async ({ url }) => {
  const pathname = url.pathname.replace(/\/+$/, "");
  if (pathname === "/api/github-stars") {
    return getGitHubStars({ env: environment });
  }
  if (pathname === "/api/releases") {
    return getGitHubReleases({ env: environment });
  }

  return new Response("Not found", { status: 404 });
};
