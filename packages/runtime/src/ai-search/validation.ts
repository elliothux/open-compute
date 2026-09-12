export const encoder = new TextEncoder();
export function fail(code = "AI_SEARCH_INPUT_INVALID"): never {
  throw new TypeError(code);
}
export function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
export function exact(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some((key) => !keys.includes(key)))
    fail();
  return value;
}
export function protocolExact(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some((key) => !keys.includes(key)))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  return value;
}
export function text(value: unknown, maximum = 8192): string {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    /\0/.test(value) ||
    encoder.encode(value).byteLength > maximum
  )
    fail();
  return value;
}
export function instanceName(value: unknown): string {
  const name = text(value, 64);
  if (!/^[a-z0-9_]+(?:-[a-z0-9_]+)*$/.test(name)) fail();
  return name;
}
export function opaqueId(value: unknown): string {
  return text(value, 256);
}
export function integer(
  value: unknown,
  minimum: number,
  maximum: number,
): number {
  if (
    !Number.isSafeInteger(value) ||
    (value as number) < minimum ||
    (value as number) > maximum
  )
    fail();
  return value as number;
}
export function number(
  value: unknown,
  minimum: number,
  maximum: number,
): number {
  if (
    typeof value !== "number" ||
    !Number.isFinite(value) ||
    value < minimum ||
    value > maximum
  )
    fail();
  return value;
}
export function optionalPage(
  value: unknown,
  keys: readonly string[],
): Record<string, unknown> {
  if (value === undefined) return {};
  const params = exact(value, keys);
  if (params.page !== undefined) integer(params.page, 1, 1_000_000);
  if (params.per_page !== undefined) integer(params.per_page, 1, 100);
  for (const key of [
    "search",
    "source",
    "metadata_filter",
    "item_id",
    "key",
    "cursor",
  ]) {
    if (params[key] !== undefined) text(params[key]);
  }
  return params;
}
function messages(value: unknown): AiSearchMessage[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > 100) fail();
  const parsed = value.map((raw) => {
    const item = exact(raw, ["role", "content"]);
    if (
      !["system", "developer", "user", "assistant", "tool"].includes(
        String(item.role),
      ) ||
      (item.content !== null &&
        (typeof item.content !== "string" ||
          encoder.encode(item.content).byteLength > 16 * 1024))
    )
      fail();
    return { role: item.role, content: item.content } as AiSearchMessage;
  });
  if (
    !parsed.some(
      (item) =>
        item.role === "user" &&
        typeof item.content === "string" &&
        item.content.length > 0,
    )
  )
    fail();
  return parsed;
}
function metadataFilter(value: unknown): Record<string, unknown> {
  if (
    !record(value) ||
    Object.keys(value).length < 1 ||
    Object.keys(value).length > 64
  )
    fail();
  const serialized = JSON.stringify(value);
  if (encoder.encode(serialized).byteLength > 2048)
    fail("AI_SEARCH_LIMIT_EXCEEDED");
  for (const [field, raw] of Object.entries(value)) {
    text(field, 256);
    if (
      raw === null ||
      typeof raw === "string" ||
      typeof raw === "boolean" ||
      (typeof raw === "number" && Number.isFinite(raw))
    )
      continue;
    const operators = exact(raw, [
      "$eq",
      "$ne",
      "$lt",
      "$lte",
      "$gt",
      "$gte",
      "$in",
      "$nin",
    ]);
    if (Object.keys(operators).length < 1 || Object.keys(operators).length > 2)
      fail();
    for (const [operator, operand] of Object.entries(operators)) {
      if (operator === "$in" || operator === "$nin") {
        if (
          !Array.isArray(operand) ||
          operand.length < 1 ||
          operand.length > 100
        )
          fail();
      } else if (
        operand !== null &&
        typeof operand !== "string" &&
        typeof operand !== "boolean" &&
        (typeof operand !== "number" || !Number.isFinite(operand))
      )
        fail();
    }
  }
  return value;
}
export function uploadMetadata(value: unknown): Record<string, string> {
  if (!record(value)) fail();
  const entries = Object.entries(value);
  if (entries.length > 5) fail("AI_SEARCH_LIMIT_EXCEEDED");
  const metadata: Record<string, string> = {};
  for (const [field, item] of entries) {
    text(field, 256);
    if (typeof item !== "string") fail();
    metadata[field] = item;
  }
  if (encoder.encode(JSON.stringify(metadata)).byteLength > 10 * 1024)
    fail("AI_SEARCH_LIMIT_EXCEEDED");
  return metadata;
}
function searchOptions(
  value: unknown,
  multi: boolean,
): AiSearchOptions | AiSearchMultiSearchOptions {
  if (value === undefined) {
    if (multi) fail();
    return {};
  }
  const options = exact(value, [
    "retrieval",
    "query_rewrite",
    "reranking",
    "instance_ids",
  ]);
  const output: Record<string, unknown> = {};
  if (multi) {
    if (
      !Array.isArray(options.instance_ids) ||
      options.instance_ids.length < 1 ||
      options.instance_ids.length > 10
    )
      fail("AI_SEARCH_LIMIT_EXCEEDED");
    output.instance_ids = options.instance_ids.map(instanceName);
  } else if (options.instance_ids !== undefined) fail();
  if (options.retrieval !== undefined) {
    const raw = exact(options.retrieval, [
      "retrieval_type",
      "fusion_method",
      "keyword_match_mode",
      "match_threshold",
      "max_num_results",
      "filters",
      "context_expansion",
      "metadata_only",
      "return_on_failure",
      "boost_by",
    ]);
    if (
      raw.retrieval_type !== undefined &&
      !["vector", "keyword", "hybrid"].includes(String(raw.retrieval_type))
    )
      fail();
    if (
      raw.fusion_method !== undefined &&
      !["max", "rrf"].includes(String(raw.fusion_method))
    )
      fail();
    if (
      raw.keyword_match_mode !== undefined &&
      !["and", "or"].includes(String(raw.keyword_match_mode))
    )
      fail();
    if (raw.match_threshold !== undefined) number(raw.match_threshold, 0, 1);
    if (raw.max_num_results !== undefined) integer(raw.max_num_results, 1, 50);
    if (raw.context_expansion !== undefined)
      integer(raw.context_expansion, 0, 3);
    for (const key of ["metadata_only", "return_on_failure"])
      if (raw[key] !== undefined && typeof raw[key] !== "boolean") fail();
    if (raw.filters !== undefined) metadataFilter(raw.filters);
    if (raw.boost_by !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
    output.retrieval = raw;
  }
  if (options.query_rewrite !== undefined) {
    const raw = exact(options.query_rewrite, [
      "enabled",
      "model",
      "rewrite_prompt",
    ]);
    if (raw.enabled !== undefined && typeof raw.enabled !== "boolean") fail();
    if (raw.model !== undefined) text(raw.model, 256);
    if (raw.rewrite_prompt !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
    output.query_rewrite = raw;
  }
  if (options.reranking !== undefined) {
    const raw = exact(options.reranking, [
      "enabled",
      "model",
      "match_threshold",
    ]);
    if (raw.enabled !== undefined && typeof raw.enabled !== "boolean") fail();
    if (raw.model !== undefined) text(raw.model, 256);
    if (raw.match_threshold !== undefined) number(raw.match_threshold, 0, 1);
    output.reranking = raw;
  }
  return output as AiSearchOptions | AiSearchMultiSearchOptions;
}
export function searchRequest(
  value: unknown,
  multi: boolean,
): Record<string, unknown> {
  const params = exact(value, ["query", "messages", "ai_search_options"]);
  if ((params.query === undefined) === (params.messages === undefined)) fail();
  return {
    ...(params.query === undefined ? {} : { query: text(params.query) }),
    ...(params.messages === undefined
      ? {}
      : { messages: messages(params.messages) }),
    ai_search_options: searchOptions(params.ai_search_options, multi),
  };
}
export function chatRequest(
  value: unknown,
  multi: boolean,
): Record<string, unknown> {
  const params = exact(value, [
    "messages",
    "model",
    "stream",
    "ai_search_options",
  ]);
  if (params.stream !== undefined && typeof params.stream !== "boolean") fail();
  return {
    messages: messages(params.messages),
    ...(params.model === undefined ? {} : { model: text(params.model, 256) }),
    ...(params.stream === undefined ? {} : { stream: params.stream }),
    ai_search_options: searchOptions(params.ai_search_options, multi),
  };
}
const CONFIG_FIELDS = [
  "id",
  "type",
  "source",
  "source_params",
  "token_id",
  "sync_interval",
  "paused",
  "rewrite_query",
  "reranking",
  "embedding_model",
  "ai_search_model",
  "rewrite_model",
  "reranking_model",
  "index_method",
  "fusion_method",
  "indexing_options",
  "retrieval_options",
  "chunk",
  "chunk_size",
  "chunk_overlap",
  "score_threshold",
  "max_num_results",
  "custom_metadata",
  "metadata",
] as const;
export function config(
  value: unknown,
  updating: boolean,
): Record<string, unknown> {
  const raw = exact(value, CONFIG_FIELDS);
  if (!updating) instanceName(raw.id);
  else if (raw.id !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
  if (raw.paused !== undefined && typeof raw.paused !== "boolean") fail();
  const sourceFields = [
    raw.type,
    raw.source,
    raw.source_params,
    raw.token_id,
    raw.sync_interval,
  ];
  if (sourceFields.some((entry) => entry !== undefined)) {
    if (raw.type !== undefined && raw.type !== "r2") {
      if (raw.type !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
      fail();
    }
    if (!updating && raw.type !== "r2") fail();
    if (!updating || raw.source !== undefined) text(raw.source, 512);
    if (raw.token_id !== undefined) {
      const token = text(raw.token_id, 36);
      if (
        !/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(
          token,
        )
      )
        fail();
    }
    if (raw.sync_interval !== undefined) {
      integer(raw.sync_interval, 900, 86_400);
      if (
        ![900, 1800, 3600, 7200, 14400, 21600, 43200, 86400].includes(
          raw.sync_interval as number,
        )
      )
        fail();
    }
    if (raw.source_params !== undefined) {
      const params = exact(raw.source_params, [
        "prefix",
        "include_items",
        "exclude_items",
        "r2_jurisdiction",
      ]);
      if (
        params.prefix !== undefined &&
        (typeof params.prefix !== "string" ||
          /\0/.test(params.prefix) ||
          encoder.encode(params.prefix).byteLength > 1024)
      )
        fail();
      for (const key of ["include_items", "exclude_items"]) {
        const patterns = params[key];
        if (patterns === undefined) continue;
        if (!Array.isArray(patterns) || patterns.length > 10)
          fail("AI_SEARCH_LIMIT_EXCEEDED");
        for (const pattern of patterns) {
          const value = text(pattern, 1024);
          if (!/^[A-Za-z0-9_.\- /?:=&%*]+$/.test(value)) fail();
        }
      }
      if (params.r2_jurisdiction !== undefined)
        text(params.r2_jurisdiction, 256);
    }
  }
  for (const key of ["rewrite_query", "reranking", "chunk"])
    if (raw[key] !== undefined && typeof raw[key] !== "boolean") fail();
  for (const key of [
    "embedding_model",
    "ai_search_model",
    "rewrite_model",
    "reranking_model",
  ])
    if (raw[key] !== undefined) text(raw[key], 256);
  if (
    raw.fusion_method !== undefined &&
    !["max", "rrf"].includes(String(raw.fusion_method))
  )
    fail();
  if (raw.index_method !== undefined) {
    const method = exact(raw.index_method, ["vector", "keyword"]);
    if (
      (method.vector !== undefined && typeof method.vector !== "boolean") ||
      (method.keyword !== undefined && typeof method.keyword !== "boolean")
    )
      fail();
  }
  if (raw.indexing_options !== undefined && raw.indexing_options !== null) {
    const indexing = exact(raw.indexing_options, ["keyword_tokenizer"]);
    if (
      indexing.keyword_tokenizer !== undefined &&
      !["porter", "trigram"].includes(String(indexing.keyword_tokenizer))
    )
      fail();
  }
  if (raw.retrieval_options !== undefined && raw.retrieval_options !== null) {
    const retrieval = exact(raw.retrieval_options, [
      "keyword_match_mode",
      "boost_by",
    ]);
    if (
      retrieval.keyword_match_mode !== undefined &&
      !["and", "or"].includes(String(retrieval.keyword_match_mode))
    )
      fail();
    if (retrieval.boost_by !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
  }
  if (raw.chunk_size !== undefined) integer(raw.chunk_size, 1, 100_000);
  if (raw.chunk_overlap !== undefined) integer(raw.chunk_overlap, 0, 30);
  if (raw.score_threshold !== undefined) number(raw.score_threshold, 0, 1);
  if (raw.max_num_results !== undefined) integer(raw.max_num_results, 1, 50);
  if (raw.custom_metadata !== undefined) {
    if (!Array.isArray(raw.custom_metadata) || raw.custom_metadata.length > 5)
      fail("AI_SEARCH_LIMIT_EXCEEDED");
    for (const entry of raw.custom_metadata) {
      const item = exact(entry, ["field_name", "data_type"]);
      text(item.field_name, 256);
      if (
        !["text", "number", "boolean", "datetime"].includes(
          String(item.data_type),
        )
      )
        fail();
    }
  }
  if (raw.metadata !== undefined && !record(raw.metadata)) fail();
  return raw;
}
