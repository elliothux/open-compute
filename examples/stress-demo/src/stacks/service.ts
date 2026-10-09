import type { Env } from "../env";
import { json } from "../lib/json";

export async function handleServiceCall(
  request: Request,
  env: Env,
): Promise<Response> {
  const url = new URL(request.url);
  const mode = url.searchParams.get("mode") ?? "fetch";

  if (mode === "rpc") {
    const product = await env.SERVICE.multiply(6, 7);
    const echoed = await env.SERVICE.echo({
      probe: "stress",
      revision: env.REVISION,
    });
    return json({
      stack: "service",
      mode,
      product,
      echoed,
    });
  }

  const response = await env.SERVICE.fetch(
    "https://service.internal/internal/ping",
    {
      method: request.method === "HEAD" ? "GET" : request.method,
      headers: { "x-stress-service": "1" },
    },
  );
  let payload: unknown;
  try {
    payload = await response.json();
  } catch {
    payload = { raw: await response.text() };
  }
  return json({
    stack: "service",
    mode: "fetch",
    status: response.status,
    payload,
  });
}
