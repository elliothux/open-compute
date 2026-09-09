import { request as requestHttp } from "node:http";
import { request as requestHttps } from "node:https";
import { MAX_OUTPUT } from "./runtime-contract.ts";

export function cloudflareDeploymentUrl(
  output: string,
  workerName: string,
): string {
  if (!/^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(workerName)) {
    throw new Error("Cloudflare Worker name is invalid");
  }
  const plain = output.replaceAll(/\u001b\[[0-9;]*m/g, "");
  const urls = [
    ...plain.matchAll(/https:\/\/[a-z0-9.-]+\.workers\.dev(?:\/[^\s]*)?/gi),
  ]
    .map((match) => match[0])
    .filter((candidate) => {
      const url = new URL(candidate);
      return (
        url.hostname.startsWith(`${workerName}.`) &&
        url.hostname.endsWith(".workers.dev")
      );
    })
    .map((candidate) => new URL(candidate).origin)
    .filter((candidate, index, values) => values.indexOf(candidate) === index);
  if (urls.length !== 1)
    throw new Error("Wrangler did not report one unambiguous workers.dev URL");
  return `${urls[0]}/`;
}

export function cloudflareWorkerMissing(output: string): boolean {
  return (
    /\[\s*code:\s*(?:10007|10090)\s*\]/i.test(output) ||
    /(?:Worker|script)[^\n]{0,80}not found/i.test(output)
  );
}

export function cloudflareTransientFailure(output: string): boolean {
  return /(?:fetch failed|connectivity issue|network connectivity problems)/i.test(
    output,
  );
}

export function observationUrl(base: string, path: string): string {
  if (!path.startsWith("/") || path.startsWith("//"))
    throw new Error("observation path must be origin-relative");
  const root = new URL(base);
  if (!root.pathname.endsWith("/")) root.pathname += "/";
  const result = new URL(path.slice(1), root);
  if (
    result.origin !== root.origin ||
    !result.pathname.startsWith(root.pathname)
  ) {
    throw new Error("observation path escapes its Worker route");
  }
  return result.href;
}

/** Issue a portable observation while preserving the explicit local route Host header. */
export async function fetchObservation(
  url: string,
  init: {
    readonly method: string;
    readonly headers: Readonly<Record<string, string>>;
    readonly body?: Uint8Array;
  },
): Promise<Response> {
  if (!("host" in init.headers)) {
    return fetch(url, {
      method: init.method,
      redirect: "error",
      signal: AbortSignal.timeout(30_000),
      headers: init.headers,
      ...(init.body === undefined ? {} : { body: init.body }),
    });
  }
  const target = new URL(url);
  if (target.protocol !== "http:" && target.protocol !== "https:") {
    throw new Error("observation URL protocol is unsupported");
  }
  return new Promise<Response>((resolveResponse, rejectResponse) => {
    const request = (target.protocol === "https:" ? requestHttps : requestHttp)(
      target,
      {
        method: init.method,
        headers: init.headers,
        signal: AbortSignal.timeout(30_000),
      },
      (incoming) => {
        const chunks: Buffer[] = [];
        let length = 0;
        incoming.on("data", (chunk: Buffer) => {
          length += chunk.length;
          if (length > MAX_OUTPUT) {
            incoming.destroy(new Error("observation response exceeds 1 MiB"));
            return;
          }
          chunks.push(chunk);
        });
        incoming.once("error", rejectResponse);
        incoming.once("end", () => {
          const status = incoming.statusCode ?? 0;
          if (status < 200 || status > 599) {
            rejectResponse(new Error("observation response status is invalid"));
            return;
          }
          if (status >= 300 && status < 400) {
            rejectResponse(new Error("observation redirect is forbidden"));
            return;
          }
          const headers = new Headers();
          for (const [name, value] of Object.entries(incoming.headers)) {
            if (Array.isArray(value))
              value.forEach((item) => headers.append(name, item));
            else if (value !== undefined) headers.set(name, value);
          }
          const withoutBody =
            status === 204 || status === 205 || status === 304;
          resolveResponse(
            new Response(withoutBody ? null : Buffer.concat(chunks), {
              status,
              headers,
            }),
          );
        });
      },
    );
    request.once("error", rejectResponse);
    request.end(init.body === undefined ? undefined : Buffer.from(init.body));
  });
}
