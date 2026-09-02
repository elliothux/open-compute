import type { OperatorTransport, OperatorRequestBody, OperatorBinaryResponse } from "../transport.js";
import { requestOptions } from "../transport.js";
import { OperatorProtocolError } from "../error.js";
import type {
  BinaryOperationDef,
  BodyJsonOperationDef,
  JsonOperationDef,
  OperationCallOptions,
} from "./types.js";

function stripTransportParams<TParams>(params: TParams): TParams {
  if (params === null || typeof params !== "object" || Array.isArray(params)) {
    return params;
  }
  const { signal: _signal, ...schemaParams } = params as TParams & { signal?: AbortSignal };
  return schemaParams as TParams;
}

function validateParams<TParams>(
  operation: { paramsSchema?: { safeParse: (value: unknown) => { success: boolean; data?: TParams } } },
  params: TParams,
): TParams {
  if (!operation.paramsSchema) {
    return params;
  }
  const parsed = operation.paramsSchema.safeParse(stripTransportParams(params));
  if (!parsed.success || parsed.data === undefined) {
    throw new OperatorProtocolError("operator request parameters did not match the input schema");
  }
  return parsed.data;
}

export function invokeJsonOperation<TParams, TResult>(
  transport: OperatorTransport,
  operation: JsonOperationDef<TParams, TResult>,
  params: TParams,
  options: OperationCallOptions = {},
): Promise<TResult> {
  const validated = validateParams(operation, params);
  const { query, body, signal, idempotencyKey, headers } = options;
  const request: Parameters<OperatorTransport["requestJson"]>[3] = {
    ...requestOptions({
      signal,
      idempotencyKey: operation.idempotent ? idempotencyKey : undefined,
      headers,
    }),
  };
  if (query !== undefined) {
    request.query = query;
  }
  if (body !== undefined) {
    request.body = body;
  }
  return transport.requestJson(operation.method, operation.path(validated), operation.successSchema, request);
}

export function invokeBinaryOperation<TParams>(
  transport: OperatorTransport,
  operation: BinaryOperationDef<TParams>,
  params: TParams,
  options: OperationCallOptions = {},
): Promise<OperatorBinaryResponse> {
  const validated = validateParams(operation, params);
  const { signal, idempotencyKey, body, contentType, maxBytes, headers } = options;
  const request: Parameters<OperatorTransport["requestBinary"]>[2] = {
    ...requestOptions({
      signal,
      idempotencyKey: operation.idempotent ? idempotencyKey : undefined,
      headers,
    }),
  };
  if (body !== undefined) {
    request.body = body as OperatorRequestBody;
  }
  if (contentType !== undefined) {
    request.contentType = contentType;
  }
  if (maxBytes !== undefined) {
    request.maxBytes = maxBytes;
  }
  return transport.requestBinary(operation.method, operation.path(validated), request);
}

export function invokeBodyJsonOperation<TParams, TResult>(
  transport: OperatorTransport,
  operation: BodyJsonOperationDef<TParams, TResult>,
  params: TParams,
  options: OperationCallOptions = {},
): Promise<TResult> {
  const validated = validateParams(operation, params);
  const { query, body, signal, idempotencyKey, contentType, headers } = options;
  const request: Parameters<OperatorTransport["requestBodyJson"]>[3] = {
    ...requestOptions({
      signal,
      idempotencyKey: operation.idempotent ? idempotencyKey : undefined,
      headers,
    }),
  };
  if (query !== undefined) {
    request.query = query;
  }
  if (body !== undefined) {
    request.body = body as OperatorRequestBody;
  }
  if (contentType !== undefined) {
    request.contentType = contentType;
  }
  return transport.requestBodyJson(operation.method, operation.path(validated), operation.successSchema, request);
}
