import type { Env } from "../env";
import { json, jsonError, readJson } from "../lib/json";

interface SpinBody {
  iterations?: number;
}

export async function handleCpuSpin(
  request: Request,
  env: Env,
): Promise<Response> {
  if (request.method !== "POST") {
    return jsonError("method_not_allowed", 405);
  }

  const url = new URL(request.url);
  const body = await readJson<SpinBody>(request);
  const iterations =
    body.iterations ?? Number(url.searchParams.get("iterations") ?? "50000");

  const started = Date.now();
  let checksum = 0;
  for (let index = 0; index < iterations; index += 1) {
    checksum = (checksum + index * 31) % 1_000_003;
  }

  return json({
    stack: "cpu",
    checksum,
    iterations,
    elapsedMs: Date.now() - started,
    revision: env.REVISION,
  });
}
