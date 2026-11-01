import { bindingJson } from "../bindings/json-body.js";
import type {
  D1RawTransport,
  D1SessionWire,
  D1StatementDto,
  D1Value,
} from "./protocol.js";
import { assertQueryResponse } from "./validation.js";

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

// Cloudflare's D1 service reports a malformed statement list with this exact message.
class D1MalformedInput extends TypeError {
  constructor(queries: unknown[]) {
    super(
      `Malformed input: ${JSON.stringify(queries)}, should be {sql: string, params?: any[]} or an array of these query objects`,
    );
  }
}

function statements(value: unknown): D1StatementDto[] {
  if (Array.isArray(value) && value.length === 0)
    throw new TypeError("D1_NO_SQL_STATEMENTS");
  if (!Array.isArray(value) || value.length > 0xffff)
    throw new TypeError("D1_TYPE_ERROR");
  return value.map((entry: unknown) => {
    if (
      !record(entry) ||
      typeof entry.sql !== "string" ||
      (entry.params !== undefined && !Array.isArray(entry.params))
    )
      throw new D1MalformedInput(value);
    const params: D1Value[] = [];
    if (Array.isArray(entry.params))
      for (const param of entry.params as unknown[]) {
        if (
          param === null ||
          typeof param === "string" ||
          (typeof param === "number" && Number.isFinite(param))
        )
          params.push(param);
        else if (
          Array.isArray(param) &&
          param.every(
            (byte: unknown) =>
              typeof byte === "number" && byte >= 0 && byte < 256,
          )
        )
          params.push(Uint8Array.from(param));
        else throw new TypeError("D1_TYPE_ERROR");
      }
    return { sql: entry.sql, params };
  });
}

function session(bookmark: unknown): D1SessionWire {
  if (bookmark === undefined || bookmark === null) return { kind: 0 };
  if (bookmark === "first-primary") return { kind: 2 };
  if (bookmark === "first-unconstrained") return { kind: 1 };
  if (typeof bookmark !== "string" || !bookmark || bookmark.length > 4096)
    throw new TypeError("D1_SESSION_ERROR");
  return { kind: 3, bookmark };
}

/** Preserve stable authority errors while hiding upstream exceptions and query contents. */
export function nativeD1Error(error: unknown): string {
  const code: unknown =
    error instanceof TypeError
      ? error.message
      : error instanceof Error
        ? Object.getOwnPropertyDescriptor(error, "stableCode")?.value
        : undefined;
  if (error instanceof D1MalformedInput) return error.message;
  if (code === "D1_NO_SQL_STATEMENTS") return "No SQL statements detected.";
  return typeof code === "string" &&
    /^(?:D1|BINDING)_[A-Z0-9_]{1,127}$/.test(code)
    ? code
    : "D1_INTERNAL_PROTOCOL_ERROR";
}

/** The pinned upstream D1 binding's query service contract, including session bookmarks. */
export async function nativeD1Query(
  authority: Pick<D1RawTransport, "executeStatements">,
  input: unknown,
  format = "ROWS_AND_COLUMNS",
) {
  if (!record(input)) throw new TypeError("D1_TYPE_ERROR");
  const queries = statements(input.queries);
  if (queries.some((query) => query.sql.trim().length === 0))
    throw new TypeError("D1_NO_SQL_STATEMENTS");
  const selectedSession = session(input.bookmark);
  const output = assertQueryResponse(
    await authority.executeStatements(
      queries.length > 1 ? "batch" : format === "NONE" ? "run" : "all",
      queries,
      selectedSession,
    ),
    selectedSession.kind !== 0,
  );
  if (output.results.length !== queries.length)
    throw new TypeError("D1_INTERNAL_PROTOCOL_ERROR");
  return {
    queryResults: output.results.map((result) => {
      return {
        meta: result.meta,
        data: {
          kind: "raw",
          columns: result.columns,
          rows: result.rows,
        },
      };
    }),
    ...(output.bookmark === null ? {} : { bookmark: output.bookmark }),
  };
}

/** HTTP contract used by upstream D1 when d1_binding_jsrpc is disabled. */
export async function nativeD1Fetch(
  request: Request,
  authority: Pick<D1RawTransport, "executeStatements" | "exec">,
): Promise<Response> {
  try {
    const url = new URL(request.url);
    if (
      request.method !== "POST" ||
      url.hostname !== "d1" ||
      !["/query", "/execute"].includes(url.pathname)
    )
      throw new TypeError("D1_TYPE_ERROR");
    let format = url.searchParams.get("resultsFormat") ?? "ARRAY_OF_OBJECTS";
    if (!["ROWS_AND_COLUMNS", "ARRAY_OF_OBJECTS", "NONE"].includes(format))
      throw new TypeError("D1_TYPE_ERROR");
    const input = await bindingJson(
      request,
      16 * 1024 * 1024,
      "D1_LIMIT_ERROR",
    );
    if (url.pathname === "/execute" && Array.isArray(input)) {
      const queries = statements(input);
      if (queries.some((query) => query.params.length !== 0))
        throw new TypeError("D1_TYPE_ERROR");
      const results = [];
      for (const query of queries) {
        try {
          if (query.sql.trim().length === 0)
            throw new TypeError("D1_NO_SQL_STATEMENTS");
          const result = await authority.exec(query.sql);
          if (
            !record(result) ||
            !Number.isSafeInteger(result.count) ||
            typeof result.count !== "number" ||
            result.count < 1 ||
            typeof result.duration !== "number" ||
            !Number.isFinite(result.duration)
          )
            throw new TypeError("D1_INTERNAL_PROTOCOL_ERROR");
          results.push({
            success: true,
            meta: { duration: result.duration },
            results: [],
          });
        } catch (error) {
          results.push({ success: false, error: nativeD1Error(error) });
          break;
        }
      }
      return Response.json(results);
    }
    // Upstream prepared run() sends one object to /execute and returns SELECT rows.
    if (url.pathname === "/execute") format = "ARRAY_OF_OBJECTS";
    const result = await nativeD1Query(
      authority,
      {
        queries: Array.isArray(input) ? input : [input],
        bookmark: request.headers.get("x-cf-d1-session-commit-token"),
      },
      format,
    );
    const rows = result.queryResults.map((query) => ({
      success: true,
      meta: query.meta,
      results:
        format === "NONE"
          ? []
          : format === "ROWS_AND_COLUMNS"
            ? { columns: query.data.columns, rows: query.data.rows }
            : query.data.rows.map((row) =>
                Object.fromEntries(
                  query.data.columns.map((column, index) => [
                    column,
                    row[index],
                  ]),
                ),
              ),
    }));
    return Response.json(Array.isArray(input) ? rows : rows[0], {
      headers:
        result.bookmark === undefined
          ? {}
          : { "x-cf-d1-session-commit-token": result.bookmark },
    });
  } catch (error) {
    return Response.json(
      { success: false, error: nativeD1Error(error) },
      { status: 400 },
    );
  }
}
