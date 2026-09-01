import { env } from "cloudflare:workers";

const SERVER_ONLY_CANARY = "P4_SERVER_ONLY_7f4a2c9e";

export function GET(): Response {
  return Response.json({
    marker: env.P4_PUBLIC_MARKER,
    serverOnlyCanaryExposed: false,
    serverOnlyCanaryLength: SERVER_ONLY_CANARY.length,
  }, {
    headers: { "cache-control": "no-store", "x-p4-env": "cloudflare-workers" },
  });
}
