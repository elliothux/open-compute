/** Values accepted by the binary D1 protocol after tenant-side normalization. */
export type D1Value = null | string | number | Uint8Array;
export type D1QueryMode = "all" | "run" | "raw" | "batch";
export interface D1StatementDto {
  sql: string;
  params: readonly D1Value[];
}

/** Session constraint or sealed bookmark sent with a terminal D1 query. */
export interface D1SessionWire {
  kind: 0 | 1 | 2 | 3;
  bookmark?: string;
}

/** The tenant facade validates responses before exposing them to user code. */
export interface D1RawTransport {
  query(
    mode: D1QueryMode,
    statements: readonly D1StatementDto[],
    session?: D1SessionWire,
  ): Promise<unknown>;
  exec(sql: string, options?: Record<string, never>): Promise<unknown>;
}
