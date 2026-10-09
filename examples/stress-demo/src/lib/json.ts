export type Json = Record<string, unknown>;
export type StackName =
  | "http"
  | "kv"
  | "d1"
  | "r2"
  | "do"
  | "queue"
  | "workflow"
  | "fetch"
  | "cpu"
  | "service"
  | "scenario";

export function json(value: Json, status = 200): Response {
  return Response.json(value, { status });
}

export function ok(value: Json, status = 200): Response {
  return json({ ok: true, ...value }, status);
}

export function structuredError(
  errorCode: string,
  stack: StackName,
  details: Json = {},
  status = 500,
): Response {
  return json({ ok: false, error_code: errorCode, stack, details }, status);
}

export function jsonError(
  message: string,
  status = 400,
  extra: Json = {},
): Response {
  const stack = (
    typeof extra.stack === "string" ? extra.stack : "http"
  ) as StackName;
  const { stack: _stack, ...details } = extra;
  return structuredError(
    message.toUpperCase(),
    stack,
    { message, ...details },
    status,
  );
}

export async function readJson<T = Json>(request: Request): Promise<T> {
  return (await request.json().catch(() => ({}))) as T;
}
