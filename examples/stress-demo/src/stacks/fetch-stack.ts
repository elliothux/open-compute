import type { Env } from "../env";
import { json } from "../lib/json";

export async function handleFetchProbe(
  request: Request,
  env: Env,
): Promise<Response> {
  const url = new URL(request.url);
  const hops = Number(url.searchParams.get("hops") ?? "1");
  const target = url.searchParams.get("target") ?? env.OUTBOUND_URL;

  const results: Array<{
    hop: number;
    status: number;
    ok: boolean;
    elapsedMs: number;
  }> = [];
  let currentTarget = target;

  for (let hop = 0; hop < hops; hop += 1) {
    const started = Date.now();
    const response = await fetch(currentTarget, {
      method: "GET",
      headers: {
        "x-stress-hop": String(hop),
        "x-stress-revision": env.REVISION,
      },
    });
    results.push({
      hop,
      status: response.status,
      ok: response.ok,
      elapsedMs: Date.now() - started,
    });
    if (!response.ok) {
      break;
    }
    if (hop + 1 < hops) {
      currentTarget = `${new URL(currentTarget).origin}/health/live`;
    }
  }

  return json({
    stack: "fetch",
    hops,
    target,
    results,
    allOk: results.every((entry) => entry.ok),
  });
}
