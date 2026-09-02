import { OperatorErrorBodySchema } from "./schemas/common.js";
import type { z } from "zod";

export class OperatorApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly requestId: string;
  readonly retryAfter: number | undefined;

  constructor(
    status: number,
    body: { code: string; message: string; requestId: string },
    retryAfter?: number,
  ) {
    super(body.message);
    this.name = "OperatorApiError";
    this.status = status;
    this.code = body.code;
    this.requestId = body.requestId;
    this.retryAfter = retryAfter;
  }
}

export class OperatorProtocolError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "OperatorProtocolError";
  }
}

export function parseOperatorError(status: number, payload: unknown, retryAfter?: number): OperatorApiError | OperatorProtocolError {
  const parsed = OperatorErrorBodySchema.safeParse(payload);
  if (!parsed.success) {
    return new OperatorProtocolError("operator response did not match the error schema");
  }
  return new OperatorApiError(status, parsed.data.error, retryAfter);
}

export async function parseJsonResponse<T>(
  response: Response,
  schema: z.ZodType<T>,
): Promise<T> {
  if (response.status === 204) {
    const parsed = schema.safeParse(undefined);
    if (parsed.success) {
      return parsed.data;
    }
    throw new OperatorProtocolError("operator response did not match the success schema");
  }
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("application/json")) {
    throw new OperatorProtocolError("operator response had an unexpected content type");
  }
  const text = await readBoundedResponseText(response, DEFAULT_JSON_MAX_BYTES);
  let payload: unknown;
  try {
    payload = JSON.parse(text);
  } catch {
    throw new OperatorProtocolError("operator response was not valid JSON");
  }
  const parsed = schema.safeParse(payload);
  if (!parsed.success) {
    throw new OperatorProtocolError("operator response did not match the success schema");
  }
  return parsed.data;
}

/** Maximum JSON body size accepted from operator endpoints. */
export const DEFAULT_JSON_MAX_BYTES = 4 * 1024 * 1024;

/** Default maximum streamed object download size for R2 operator downloads. */
export const DEFAULT_BINARY_MAX_BYTES = 5 * 1024 * 1024 * 1024 - 5 * 1024 * 1024;

export function parseContentLength(value: string | null): number | undefined {
  if (!value) return undefined;
  const parsed = Number.parseInt(value, 10);
  return Number.isFinite(parsed) && parsed >= 0 ? parsed : undefined;
}

export async function readBoundedResponseText(
  response: Response,
  maxBytes: number,
): Promise<string> {
  const bytes = await readBoundedResponseBytes(response, maxBytes);
  return new TextDecoder().decode(bytes);
}

export async function readBoundedResponseBytes(
  response: Response,
  maxBytes: number,
): Promise<Uint8Array> {
  const contentLength = parseContentLength(response.headers.get("content-length"));
  if (contentLength !== undefined && contentLength > maxBytes) {
    throw new OperatorProtocolError("operator response exceeded the bounded size");
  }
  const stream = response.body;
  if (!stream) {
    return new Uint8Array();
  }
  return readBoundedStreamBytes(stream, maxBytes);
}

export async function readBoundedStreamBytes(
  stream: ReadableStream<Uint8Array>,
  maxBytes: number,
): Promise<Uint8Array> {
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel();
        throw new OperatorProtocolError("operator response exceeded the bounded size");
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }
  if (chunks.length === 1) {
    return chunks[0]!;
  }
  const merged = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    merged.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return merged;
}

export function createBoundedByteStream(
  source: ReadableStream<Uint8Array>,
  maxBytes: number,
): ReadableStream<Uint8Array> {
  let total = 0;
  const reader = source.getReader();
  return new ReadableStream<Uint8Array>({
    async pull(controller) {
      const { done, value } = await reader.read();
      if (done) {
        controller.close();
        return;
      }
      total += value.byteLength;
      if (total > maxBytes) {
        await reader.cancel();
        controller.error(
          new OperatorProtocolError("operator binary response exceeds limit"),
        );
        return;
      }
      controller.enqueue(value);
    },
    cancel(reason) {
      return reader.cancel(reason);
    },
  });
}

export async function parseOperatorErrorResponse(
  response: Response,
  maxBytes = DEFAULT_JSON_MAX_BYTES,
): Promise<OperatorApiError | OperatorProtocolError> {
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.includes("application/json")) {
    return new OperatorProtocolError("operator error response had an unexpected content type");
  }
  let payload: unknown;
  try {
    const text = await readBoundedResponseText(response, maxBytes);
    payload = JSON.parse(text);
  } catch (error) {
    if (error instanceof OperatorProtocolError) {
      return error;
    }
    return new OperatorProtocolError("operator error response was not valid JSON");
  }
  const retryAfterHeader = response.headers.get("retry-after");
  const retryAfter = retryAfterHeader ? Number.parseInt(retryAfterHeader, 10) : undefined;
  return parseOperatorError(
    response.status,
    payload,
    Number.isFinite(retryAfter) ? retryAfter : undefined,
  );
}
