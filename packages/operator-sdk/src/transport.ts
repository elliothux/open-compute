import {
  DEFAULT_BINARY_MAX_BYTES,
  OperatorApiError,
  OperatorProtocolError,
  createBoundedByteStream,
  parseContentLength,
  parseJsonResponse,
  parseOperatorErrorResponse,
} from "./error.js";
import type { z } from "zod";

/** Portable request body type that does not require DOM lib in SDK consumers. */
export type OperatorRequestBody =
  | string
  | Uint8Array
  | ArrayBuffer
  | ReadableStream<Uint8Array>;

export interface OperatorTransportOptions {
  baseUrl: URL;
  getAccessToken: () => string | null | undefined;
  fetch?: typeof globalThis.fetch;
}

export interface RequestOptions {
  signal?: AbortSignal | undefined;
  idempotencyKey?: string | undefined;
  headers?: Record<string, string> | undefined;
}

/** Bounded binary download returned by operator streaming endpoints. */
export interface OperatorBinaryResponse {
  headers: Headers;
  contentLength: number | undefined;
  body: ReadableStream<Uint8Array>;
}

export function requestOptions(options: RequestOptions = {}): RequestOptions {
  const result: RequestOptions = {};
  if (options.signal) result.signal = options.signal;
  if (options.idempotencyKey) result.idempotencyKey = options.idempotencyKey;
  if (options.headers) result.headers = options.headers;
  return result;
}

export class OperatorTransport {
  readonly #baseUrl: URL;
  readonly #getAccessToken: () => string | null | undefined;
  readonly #fetch: typeof globalThis.fetch;

  constructor(options: OperatorTransportOptions) {
    if (!options.baseUrl.pathname.endsWith("/")) {
      throw new OperatorProtocolError("baseUrl must end with a trailing slash");
    }
    if (!options.baseUrl.pathname.endsWith("/operator/api/v1/")) {
      throw new OperatorProtocolError("baseUrl must use /operator/api/v1/ as the API root");
    }
    this.#baseUrl = options.baseUrl;
    this.#getAccessToken = options.getAccessToken;
    this.#fetch = options.fetch ?? globalThis.fetch.bind(globalThis);
  }

  async requestJson<T>(
    method: string,
    path: string,
    schema: z.ZodType<T>,
    options: RequestOptions & {
      query?: Record<string, string | number | boolean | undefined>;
      body?: unknown;
    } = {},
  ): Promise<T> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    if (options.query) {
      for (const [key, value] of Object.entries(options.query)) {
        if (value !== undefined) url.searchParams.set(key, String(value));
      }
    }
    const token = this.#getAccessToken();
    if (!token) {
      throw new OperatorApiError(401, {
        code: "admin_auth_required",
        message: "admin token is required",
        requestId: "client",
      });
    }
    const headers = new Headers({
      Accept: "application/json",
      Authorization: `Bearer ${token}`,
    });
    if (options.idempotencyKey) {
      headers.set("idempotency-key", options.idempotencyKey);
    }
    if (options.headers) {
      for (const [key, value] of Object.entries(options.headers)) {
        headers.set(key, value);
      }
    }
    const init: RequestInit = {
      method,
      headers,
    };
    if (options.body !== undefined) {
      headers.set("Content-Type", "application/json");
      init.body = JSON.stringify(options.body);
    }
    if (options.signal) init.signal = options.signal;
    const response = await this.#fetch(url, init);
    if (!response.ok) {
      const error = await parseOperatorErrorResponse(response);
      throw error;
    }
    return parseJsonResponse(response, schema);
  }

  async requestBinary(
    method: string,
    path: string,
    options: RequestOptions & {
      maxBytes?: number;
      body?: OperatorRequestBody;
      contentType?: string;
    } = {},
  ): Promise<OperatorBinaryResponse> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    const token = this.#getAccessToken();
    if (!token) {
      throw new OperatorApiError(401, {
        code: "admin_auth_required",
        message: "admin token is required",
        requestId: "client",
      });
    }
    const headers = new Headers({
      Authorization: `Bearer ${token}`,
    });
    if (options.idempotencyKey) {
      headers.set("idempotency-key", options.idempotencyKey);
    }
    if (options.headers) {
      for (const [key, value] of Object.entries(options.headers)) {
        headers.set(key, value);
      }
    }
    if (options.contentType) {
      headers.set("Content-Type", options.contentType);
    }
    const init: RequestInit = { method, headers };
    if (options.body !== undefined) init.body = options.body as BodyInit;
    if (options.signal) init.signal = options.signal;
    const response = await this.#fetch(url, init);
    if (!response.ok) {
      const error = await parseOperatorErrorResponse(response);
      throw error;
    }
    const maxBytes = options.maxBytes ?? DEFAULT_BINARY_MAX_BYTES;
    const contentLength = parseContentLength(response.headers.get("content-length"));
    if (contentLength !== undefined && contentLength > maxBytes) {
      throw new OperatorProtocolError("operator binary response exceeds limit");
    }
    const body = response.body
      ? createBoundedByteStream(response.body, maxBytes)
      : new ReadableStream<Uint8Array>();
    return { headers: response.headers, contentLength, body };
  }

  async requestBodyJson<T>(
    method: string,
    path: string,
    schema: z.ZodType<T>,
    options: RequestOptions & {
      body?: OperatorRequestBody;
      contentType?: string;
      query?: Record<string, string | number | boolean | undefined>;
    } = {},
  ): Promise<T> {
    const url = new URL(path.replace(/^\//, ""), this.#baseUrl);
    if (options.query) {
      for (const [key, value] of Object.entries(options.query)) {
        if (value !== undefined) url.searchParams.set(key, String(value));
      }
    }
    const token = this.#getAccessToken();
    if (!token) {
      throw new OperatorApiError(401, {
        code: "admin_auth_required",
        message: "admin token is required",
        requestId: "client",
      });
    }
    const headers = new Headers({
      Accept: "application/json",
      Authorization: `Bearer ${token}`,
    });
    if (options.idempotencyKey) {
      headers.set("idempotency-key", options.idempotencyKey);
    }
    if (options.headers) {
      for (const [key, value] of Object.entries(options.headers)) {
        headers.set(key, value);
      }
    }
    if (options.contentType) {
      headers.set("Content-Type", options.contentType);
    }
    const init: RequestInit = { method, headers };
    if (options.body !== undefined) init.body = options.body as BodyInit;
    if (options.signal) init.signal = options.signal;
    const response = await this.#fetch(url, init);
    if (!response.ok) {
      const error = await parseOperatorErrorResponse(response);
      throw error;
    }
    return parseJsonResponse(response, schema);
  }
}
