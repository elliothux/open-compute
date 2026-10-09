import { WorkerEntrypoint } from "cloudflare:workers";
import type { Env } from "./env";

export class InternalApi extends WorkerEntrypoint<Env> {
  override fetch(request: Request): Response {
    const url = new URL(request.url);
    if (url.pathname === "/internal/ping") {
      return Response.json({
        entrypoint: "internal",
        revision: this.env.REVISION,
        method: request.method,
      });
    }
    return Response.json(
      { error: "not_found", path: url.pathname },
      { status: 404 },
    );
  }

  echo(value: unknown): { value: unknown; revision: string } {
    return { value, revision: this.env.REVISION };
  }

  multiply(a: number, b: number): number {
    return a * b;
  }
}
