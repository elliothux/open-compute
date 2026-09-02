import type { z } from "zod";
import type { RequestOptions, OperatorRequestBody } from "../transport.js";

export type HttpMethod = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

export interface JsonOperationDef<TParams, TResult> {
  readonly id: string;
  readonly method: HttpMethod;
  readonly path: (params: TParams) => string;
  readonly successSchema: z.ZodType<TResult>;
  readonly paramsSchema?: z.ZodType<TParams>;
  readonly idempotent?: boolean;
}

export interface BinaryOperationDef<TParams> {
  readonly id: string;
  readonly method: HttpMethod;
  readonly path: (params: TParams) => string;
  readonly paramsSchema?: z.ZodType<TParams>;
  readonly idempotent?: boolean;
}

export interface BodyJsonOperationDef<TParams, TResult> {
  readonly id: string;
  readonly method: HttpMethod;
  readonly path: (params: TParams) => string;
  readonly successSchema: z.ZodType<TResult>;
  readonly paramsSchema?: z.ZodType<TParams>;
  readonly idempotent?: boolean;
}

export type OperationCallOptions = RequestOptions & {
  query?: Record<string, string | number | boolean | undefined>;
  body?: OperatorRequestBody | unknown;
  contentType?: string;
  maxBytes?: number;
};
