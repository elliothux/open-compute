interface AiSearchTransport {
  call(operation: string, instance: string | undefined, payload: unknown): Promise<unknown>;
  stream(operation: string, instance: string | undefined, payload: unknown): Promise<Response>;
  upload(instance: string | undefined, name: string, contentType: string, body: ReadableStream<Uint8Array>, options: unknown): Promise<unknown>;
  download(instance: string | undefined, itemId: string): Promise<Response>;
}
function isTransport(value: unknown): value is AiSearchTransport {
  return value !== null && typeof value === "object" && typeof Reflect.get(value, "call") === "function"
    && typeof Reflect.get(value, "stream") === "function" && typeof Reflect.get(value, "upload") === "function"
    && typeof Reflect.get(value, "download") === "function";
}

const encoder = new TextEncoder();
function fail(code = "AI_SEARCH_INPUT_INVALID"): never { throw new TypeError(code); }
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function exact(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some(key => !keys.includes(key))) fail();
  return value;
}
function protocolExact(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some(key => !keys.includes(key))) fail("AI_SEARCH_PROTOCOL_ERROR");
  return value;
}
function text(value: unknown, maximum = 8192): string {
  if (typeof value !== "string" || value.length === 0 || /\0/.test(value) || encoder.encode(value).byteLength > maximum) fail();
  return value;
}
function instanceName(value: unknown): string {
  const name = text(value, 32);
  if (!/^[a-z0-9_]+(?:-[a-z0-9_]+)*$/.test(name)) fail();
  return name;
}
function opaqueId(value: unknown): string { return text(value, 256); }
function integer(value: unknown, minimum: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum || (value as number) > maximum) fail();
  return value as number;
}
function number(value: unknown, minimum: number, maximum: number): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < minimum || value > maximum) fail();
  return value;
}
function optionalPage(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (value === undefined) return {};
  const params = exact(value, keys);
  if (params.page !== undefined) integer(params.page, 1, 1_000_000);
  if (params.per_page !== undefined) integer(params.per_page, 1, 100);
  for (const key of ["search", "source", "metadata_filter", "item_id", "key", "cursor"]) {
    if (params[key] !== undefined) text(params[key]);
  }
  return params;
}
function messages(value: unknown): AiSearchMessage[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > 100) fail();
  const parsed = value.map(raw => {
    const item = exact(raw, ["role", "content"]);
    if (!["system", "developer", "user", "assistant", "tool"].includes(String(item.role))
        || (item.content !== null && (typeof item.content !== "string" || encoder.encode(item.content).byteLength > 16 * 1024))) fail();
    return { role: item.role, content: item.content } as AiSearchMessage;
  });
  if (!parsed.some(item => item.role === "user" && typeof item.content === "string" && item.content.length > 0)) fail();
  return parsed;
}
function metadataFilter(value: unknown): Record<string, unknown> {
  if (!record(value) || Object.keys(value).length < 1 || Object.keys(value).length > 64) fail();
  const serialized = JSON.stringify(value);
  if (encoder.encode(serialized).byteLength > 2048) fail("AI_SEARCH_LIMIT_EXCEEDED");
  for (const [field, raw] of Object.entries(value)) {
    text(field, 256);
    if (raw === null || typeof raw === "string" || typeof raw === "boolean"
        || (typeof raw === "number" && Number.isFinite(raw))) continue;
    const operators = exact(raw, ["$eq", "$ne", "$lt", "$lte", "$gt", "$gte", "$in", "$nin"]);
    if (Object.keys(operators).length < 1 || Object.keys(operators).length > 2) fail();
    for (const [operator, operand] of Object.entries(operators)) {
      if (operator === "$in" || operator === "$nin") {
        if (!Array.isArray(operand) || operand.length < 1 || operand.length > 100) fail();
      } else if (operand !== null && typeof operand !== "string" && typeof operand !== "boolean"
          && (typeof operand !== "number" || !Number.isFinite(operand))) fail();
    }
  }
  return value;
}
function uploadMetadata(value: unknown): Record<string, string> {
  if (!record(value)) fail();
  const entries = Object.entries(value);
  if (entries.length > 5) fail("AI_SEARCH_LIMIT_EXCEEDED");
  const metadata: Record<string, string> = {};
  for (const [field, item] of entries) {
    text(field, 256);
    if (typeof item !== "string") fail();
    metadata[field] = item;
  }
  if (encoder.encode(JSON.stringify(metadata)).byteLength > 10 * 1024) fail("AI_SEARCH_LIMIT_EXCEEDED");
  return metadata;
}
function searchOptions(value: unknown, multi: boolean): AiSearchOptions | AiSearchMultiSearchOptions {
  if (value === undefined) { if (multi) fail(); return {}; }
  const options = exact(value, ["retrieval", "query_rewrite", "reranking", "instance_ids"]);
  const output: Record<string, unknown> = {};
  if (multi) {
    if (!Array.isArray(options.instance_ids) || options.instance_ids.length < 1 || options.instance_ids.length > 10) fail("AI_SEARCH_LIMIT_EXCEEDED");
    output.instance_ids = options.instance_ids.map(instanceName);
  } else if (options.instance_ids !== undefined) fail();
  if (options.retrieval !== undefined) {
    const raw = exact(options.retrieval, ["retrieval_type", "fusion_method", "keyword_match_mode", "match_threshold", "max_num_results", "filters", "context_expansion", "metadata_only", "return_on_failure", "boost_by"]);
    if (raw.retrieval_type !== undefined && !["vector", "keyword", "hybrid"].includes(String(raw.retrieval_type))) fail();
    if (raw.fusion_method !== undefined && !["max", "rrf"].includes(String(raw.fusion_method))) fail();
    if (raw.keyword_match_mode !== undefined && !["and", "or"].includes(String(raw.keyword_match_mode))) fail();
    if (raw.match_threshold !== undefined) number(raw.match_threshold, 0, 1);
    if (raw.max_num_results !== undefined) integer(raw.max_num_results, 1, 50);
    if (raw.context_expansion !== undefined) integer(raw.context_expansion, 0, 3);
    for (const key of ["metadata_only", "return_on_failure"]) if (raw[key] !== undefined && typeof raw[key] !== "boolean") fail();
    if (raw.filters !== undefined) metadataFilter(raw.filters);
    if (raw.boost_by !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
    output.retrieval = raw;
  }
  if (options.query_rewrite !== undefined) {
    const raw = exact(options.query_rewrite, ["enabled", "model", "rewrite_prompt"]);
    if (raw.enabled !== undefined && typeof raw.enabled !== "boolean") fail();
    if (raw.model !== undefined) text(raw.model, 256);
    if (raw.rewrite_prompt !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
    output.query_rewrite = raw;
  }
  if (options.reranking !== undefined) {
    const raw = exact(options.reranking, ["enabled", "model", "match_threshold"]);
    if (raw.enabled !== undefined && typeof raw.enabled !== "boolean") fail();
    if (raw.model !== undefined) text(raw.model, 256);
    if (raw.match_threshold !== undefined) number(raw.match_threshold, 0, 1);
    output.reranking = raw;
  }
  return output as AiSearchOptions | AiSearchMultiSearchOptions;
}
function searchRequest(value: unknown, multi: boolean): Record<string, unknown> {
  const params = exact(value, ["query", "messages", "ai_search_options"]);
  if ((params.query === undefined) === (params.messages === undefined)) fail();
  return {
    ...(params.query === undefined ? {} : { query: text(params.query) }),
    ...(params.messages === undefined ? {} : { messages: messages(params.messages) }),
    ai_search_options: searchOptions(params.ai_search_options, multi),
  };
}
function chatRequest(value: unknown, multi: boolean): Record<string, unknown> {
  const params = exact(value, ["messages", "model", "stream", "ai_search_options"]);
  if (params.stream !== undefined && typeof params.stream !== "boolean") fail();
  return {
    messages: messages(params.messages),
    ...(params.model === undefined ? {} : { model: text(params.model, 256) }),
    ...(params.stream === undefined ? {} : { stream: params.stream }),
    ai_search_options: searchOptions(params.ai_search_options, multi),
  };
}
const CONFIG_FIELDS = ["id", "rewrite_query", "reranking", "embedding_model", "ai_search_model", "rewrite_model", "reranking_model", "index_method", "fusion_method", "indexing_options", "retrieval_options", "chunk", "chunk_size", "chunk_overlap", "score_threshold", "max_num_results", "custom_metadata", "metadata"] as const;
function config(value: unknown, updating: boolean): Record<string, unknown> {
  const raw = exact(value, CONFIG_FIELDS);
  if (!updating) instanceName(raw.id); else if (raw.id !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
  for (const key of ["rewrite_query", "reranking", "chunk"]) if (raw[key] !== undefined && typeof raw[key] !== "boolean") fail();
  for (const key of ["embedding_model", "ai_search_model", "rewrite_model", "reranking_model"]) if (raw[key] !== undefined) text(raw[key], 256);
  if (raw.fusion_method !== undefined && !["max", "rrf"].includes(String(raw.fusion_method))) fail();
  if (raw.index_method !== undefined) {
    const method = exact(raw.index_method, ["vector", "keyword"]);
    if ((method.vector !== undefined && typeof method.vector !== "boolean") || (method.keyword !== undefined && typeof method.keyword !== "boolean")) fail();
  }
  if (raw.indexing_options !== undefined && raw.indexing_options !== null) {
    const indexing = exact(raw.indexing_options, ["keyword_tokenizer"]);
    if (indexing.keyword_tokenizer !== undefined && !["porter", "trigram"].includes(String(indexing.keyword_tokenizer))) fail();
  }
  if (raw.retrieval_options !== undefined && raw.retrieval_options !== null) {
    const retrieval = exact(raw.retrieval_options, ["keyword_match_mode", "boost_by"]);
    if (retrieval.keyword_match_mode !== undefined && !["and", "or"].includes(String(retrieval.keyword_match_mode))) fail();
    if (retrieval.boost_by !== undefined) fail("AI_SEARCH_OPTION_UNSUPPORTED");
  }
  if (raw.chunk_size !== undefined) integer(raw.chunk_size, 1, 100_000);
  if (raw.chunk_overlap !== undefined) integer(raw.chunk_overlap, 0, 30);
  if (raw.score_threshold !== undefined) number(raw.score_threshold, 0, 1);
  if (raw.max_num_results !== undefined) integer(raw.max_num_results, 1, 50);
  if (raw.custom_metadata !== undefined) {
    if (!Array.isArray(raw.custom_metadata) || raw.custom_metadata.length > 5) fail("AI_SEARCH_LIMIT_EXCEEDED");
    for (const entry of raw.custom_metadata) {
      const item = exact(entry, ["field_name", "data_type"]); text(item.field_name, 256);
      if (!["text", "number", "boolean", "datetime"].includes(String(item.data_type))) fail();
    }
  }
  if (raw.metadata !== undefined && !record(raw.metadata)) fail();
  return raw;
}

function json(value: unknown, depth = 0): boolean {
  if (depth > 16) return false;
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.length <= 10_000 && value.every(item => json(item, depth + 1));
  return record(value) && Object.keys(value).length <= 1_000 && Object.values(value).every(item => json(item, depth + 1));
}
function pagination(value: unknown): void {
  if (value === undefined) return;
  const info = protocolExact(value, ["count", "page", "per_page", "total_count"]);
  for (const key of ["count", "page", "per_page", "total_count"]) integer(info[key], key === "page" ? 1 : 0, 1_000_000_000);
}
function instanceInfo(value: unknown): AiSearchInstanceInfo {
  const info = protocolExact(value, ["id", "type", "source", "source_params", "paused", "status", "namespace", "created_at", "modified_at", "token_id", "ai_gateway_id", "rewrite_query", "reranking", "embedding_model", "ai_search_model", "rewrite_model", "reranking_model", "hybrid_search_enabled", "index_method", "fusion_method", "indexing_options", "retrieval_options", "chunk", "chunk_size", "chunk_overlap", "score_threshold", "max_num_results", "cache", "cache_threshold", "custom_metadata", "sync_interval", "metadata"]);
  instanceName(info.id);
  for (const key of ["type", "source", "status", "namespace", "created_at", "modified_at", "token_id", "ai_gateway_id", "embedding_model", "ai_search_model", "rewrite_model", "reranking_model"]) if (info[key] !== undefined && typeof info[key] !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["paused", "rewrite_query", "reranking", "hybrid_search_enabled", "chunk", "cache"]) if (info[key] !== undefined && typeof info[key] !== "boolean") fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["chunk_size", "chunk_overlap", "score_threshold", "max_num_results", "sync_interval"]) if (info[key] !== undefined && (typeof info[key] !== "number" || !Number.isFinite(info[key]))) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["source_params", "index_method", "indexing_options", "retrieval_options", "custom_metadata", "metadata"]) if (info[key] !== undefined && !json(info[key])) fail("AI_SEARCH_PROTOCOL_ERROR");
  return info as AiSearchInstanceInfo;
}
function itemInfo(value: unknown): AiSearchItemInfo {
  const info = protocolExact(value, ["id", "key", "status", "next_action", "error", "checksum", "namespace", "chunks_count", "file_size", "source_id", "last_seen_at", "created_at", "metadata"]);
  opaqueId(info.id); text(info.key, 1024);
  if (!["completed", "error", "skipped", "queued", "running", "outdated"].includes(String(info.status))) fail("AI_SEARCH_PROTOCOL_ERROR");
  if (info.next_action !== undefined && info.next_action !== null && !["INDEX", "DELETE"].includes(String(info.next_action))) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["error", "checksum", "namespace", "source_id", "last_seen_at", "created_at"]) if (info[key] !== undefined && info[key] !== null && typeof info[key] !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["chunks_count", "file_size"]) if (info[key] !== undefined && info[key] !== null && (!Number.isSafeInteger(info[key]) || (info[key] as number) < 0)) fail("AI_SEARCH_PROTOCOL_ERROR");
  if (info.metadata !== undefined && !json(info.metadata)) fail("AI_SEARCH_PROTOCOL_ERROR");
  return info as AiSearchItemInfo;
}
function chunk(value: unknown, multi: boolean): AiSearchSearchResponse["chunks"][number] {
  const raw = protocolExact(value, ["id", "type", "score", "text", "item", "scoring_details", ...(multi ? ["instance_id"] : [])]);
  opaqueId(raw.id); text(raw.type, 128); number(raw.score, 0, 1); if (typeof raw.text !== "string" || encoder.encode(raw.text).byteLength > 1024 * 1024) fail("AI_SEARCH_PROTOCOL_ERROR");
  const item = protocolExact(raw.item, ["timestamp", "key", "metadata"]); text(item.key, 1024);
  if (item.timestamp !== undefined && (typeof item.timestamp !== "number" || !Number.isFinite(item.timestamp))) fail("AI_SEARCH_PROTOCOL_ERROR");
  if (item.metadata !== undefined && !json(item.metadata)) fail("AI_SEARCH_PROTOCOL_ERROR");
  if (raw.scoring_details !== undefined && !json(raw.scoring_details)) fail("AI_SEARCH_PROTOCOL_ERROR");
  if (multi && typeof raw.instance_id !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchSearchResponse["chunks"][number];
}
function searchResponse(value: unknown, multi: boolean): AiSearchSearchResponse | AiSearchMultiSearchResponse {
  const raw = protocolExact(value, ["search_query", "chunks", ...(multi ? ["errors"] : [])]);
  if (typeof raw.search_query !== "string" || !Array.isArray(raw.chunks) || raw.chunks.length > 50) fail("AI_SEARCH_PROTOCOL_ERROR");
  raw.chunks.map(item => chunk(item, multi));
  if (multi && raw.errors !== undefined) {
    if (!Array.isArray(raw.errors) || raw.errors.length > 10) fail("AI_SEARCH_PROTOCOL_ERROR");
    for (const error of raw.errors) { const item = protocolExact(error, ["instance_id", "message"]); instanceName(item.instance_id); text(item.message, 4096); }
  }
  return raw as AiSearchSearchResponse | AiSearchMultiSearchResponse;
}
function chatResponse(value: unknown, multi: boolean): AiSearchChatCompletionsResponse | AiSearchMultiChatCompletionsResponse {
  const raw = protocolExact(value, ["id", "object", "model", "choices", "chunks", ...(multi ? ["errors"] : [])]);
  for (const key of ["id", "object", "model"]) if (raw[key] !== undefined && typeof raw[key] !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  if (!Array.isArray(raw.choices) || raw.choices.length > 100 || !Array.isArray(raw.chunks) || raw.chunks.length > 50) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const choice of raw.choices) {
    const item = protocolExact(choice, ["index", "message"]); if (item.index !== undefined) integer(item.index, 0, 1000);
    const message = protocolExact(item.message, ["role", "content"]); if (!["system", "developer", "user", "assistant", "tool"].includes(String(message.role)) || (message.content !== null && typeof message.content !== "string")) fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  raw.chunks.map(item => chunk(item, multi));
  if (multi && raw.errors !== undefined) {
    if (!Array.isArray(raw.errors) || raw.errors.length > 10) fail("AI_SEARCH_PROTOCOL_ERROR");
    for (const error of raw.errors) { const item = protocolExact(error, ["instance_id", "message"]); instanceName(item.instance_id); text(item.message, 4096); }
  }
  return raw as AiSearchChatCompletionsResponse | AiSearchMultiChatCompletionsResponse;
}
function stats(value: unknown): AiSearchStatsResponse {
  const raw = protocolExact(value, ["queued", "running", "completed", "error", "skipped", "outdated", "last_activity", "engine"]);
  for (const key of ["queued", "running", "completed", "error", "skipped", "outdated"]) if (raw[key] !== undefined) integer(raw[key], 0, 1_000_000_000);
  if (raw.last_activity !== undefined && typeof raw.last_activity !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  if (raw.engine !== undefined && !json(raw.engine)) fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchStatsResponse;
}
function itemLogs(value: unknown): AiSearchItemLogsResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) { const item = protocolExact(entry, ["timestamp", "action", "message", "fileKey", "chunkCount", "processingTimeMs", "errorType"]); text(item.timestamp); text(item.action); if (typeof item.message !== "string") fail("AI_SEARCH_PROTOCOL_ERROR"); }
  const info = protocolExact(raw.result_info, ["count", "per_page", "cursor", "truncated"]); integer(info.count, 0, 1_000_000_000); integer(info.per_page, 0, 100); if (info.cursor !== null && typeof info.cursor !== "string") fail("AI_SEARCH_PROTOCOL_ERROR"); if (typeof info.truncated !== "boolean") fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchItemLogsResponse;
}
function itemChunks(value: unknown): AiSearchItemChunksResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) {
    const item = protocolExact(entry, ["id", "text", "start_byte", "end_byte", "item"]); opaqueId(item.id);
    if (typeof item.text !== "string") fail("AI_SEARCH_PROTOCOL_ERROR"); integer(item.start_byte, 0, 1_000_000_000); integer(item.end_byte, 0, 1_000_000_000);
    const source = protocolExact(item.item, ["timestamp", "key", "metadata"]); text(source.key, 1024);
    if (source.timestamp !== undefined && (typeof source.timestamp !== "number" || !Number.isFinite(source.timestamp))) fail("AI_SEARCH_PROTOCOL_ERROR");
    if (source.metadata !== undefined && !json(source.metadata)) fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  const info = protocolExact(raw.result_info, ["count", "total", "limit", "offset"]); for (const key of ["count", "total", "limit", "offset"]) integer(info[key], 0, 1_000_000_000);
  return raw as AiSearchItemChunksResponse;
}
function jobInfo(value: unknown): AiSearchJobInfo {
  const raw = protocolExact(value, ["id", "source", "description", "last_seen_at", "started_at", "ended_at", "end_reason"]); opaqueId(raw.id); if (!["user", "schedule"].includes(String(raw.source))) fail("AI_SEARCH_PROTOCOL_ERROR"); for (const key of ["description", "last_seen_at", "started_at", "ended_at", "end_reason"]) if (raw[key] !== undefined && raw[key] !== null && typeof raw[key] !== "string") fail("AI_SEARCH_PROTOCOL_ERROR"); return raw as AiSearchJobInfo;
}
function jobLogs(value: unknown): AiSearchJobLogsResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) { const item = protocolExact(entry, ["id", "message", "message_type", "created_at"]); integer(item.id, 0, 1_000_000_000); if (typeof item.message !== "string") fail("AI_SEARCH_PROTOCOL_ERROR"); integer(item.message_type, 0, 1_000_000); if (typeof item.created_at !== "number" || !Number.isFinite(item.created_at)) fail("AI_SEARCH_PROTOCOL_ERROR"); }
  pagination(raw.result_info); return raw as AiSearchJobLogsResponse;
}
function instanceList(value: unknown): AiSearchListResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR"); raw.result.map(instanceInfo); pagination(raw.result_info); return raw as AiSearchListResponse;
}
function itemList(value: unknown): AiSearchListItemsResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR"); raw.result.map(itemInfo); pagination(raw.result_info); return raw as AiSearchListItemsResponse;
}
function jobList(value: unknown): AiSearchListJobsResponse {
  const raw = protocolExact(value, ["result", "result_info"]); if (!Array.isArray(raw.result) || raw.result.length > 100) fail("AI_SEARCH_PROTOCOL_ERROR"); raw.result.map(jobInfo); pagination(raw.result_info); return raw as AiSearchListJobsResponse;
}
async function eventStream(response: Promise<Response>): Promise<ReadableStream> {
  const value = await response;
  if (!(value instanceof Response) || value.status !== 200 || value.body === null
      || value.headers.get("content-type")?.split(";", 1)[0]?.trim().toLowerCase() !== "text/event-stream") fail("AI_SEARCH_PROTOCOL_ERROR");
  return value.body;
}

class ItemBinding {
  readonly #transport: AiSearchTransport; readonly #instance: string | undefined; readonly #id: string;
  constructor(transport: AiSearchTransport, instance: string | undefined, id: string) { this.#transport = transport; this.#instance = instance; this.#id = opaqueId(id); }
  async info(): Promise<AiSearchItemInfo> { return itemInfo(await this.#transport.call("item.info", this.#instance, { itemId: this.#id })); }
  async download(): Promise<AiSearchItemContentResult> {
    const response = await this.#transport.download(this.#instance, this.#id);
    const filename = response.headers.get("x-open-compute-filename"); const contentType = response.headers.get("content-type");
    const size = Number(response.headers.get("content-length"));
    if (!response.ok || response.body === null || filename === null || contentType === null || !Number.isSafeInteger(size) || size < 0) fail("AI_SEARCH_PROTOCOL_ERROR");
    return { body: response.body, filename, contentType, size };
  }
  async sync(): Promise<AiSearchItemInfo> { return itemInfo(await this.#transport.call("item.sync", this.#instance, { itemId: this.#id })); }
  async logs(params?: AiSearchItemLogsParams): Promise<AiSearchItemLogsResponse> {
    const value = optionalPage(params, ["limit", "cursor"]); if (value.limit !== undefined) integer(value.limit, 1, 100);
    return itemLogs(await this.#transport.call("item.logs", this.#instance, { itemId: this.#id, params: value }));
  }
  async chunks(params?: AiSearchItemChunksParams): Promise<AiSearchItemChunksResponse> {
    const value = params === undefined ? {} : exact(params, ["limit", "offset"]);
    if (value.limit !== undefined) integer(value.limit, 1, 100); if (value.offset !== undefined) integer(value.offset, 0, 1_000_000_000);
    return itemChunks(await this.#transport.call("item.chunks", this.#instance, { itemId: this.#id, params: value }));
  }
}
class ItemsBinding {
  readonly #transport: AiSearchTransport; readonly #instance: string | undefined;
  constructor(transport: AiSearchTransport, instance: string | undefined) { this.#transport = transport; this.#instance = instance; }
  async list(params?: AiSearchListItemsParams): Promise<AiSearchListItemsResponse> {
    const value = optionalPage(params, ["page", "per_page", "search", "sort_by", "status", "source", "metadata_filter", "item_id", "key"]);
    if (value.sort_by !== undefined && !["status", "modified_at"].includes(String(value.sort_by))) fail();
    if (value.status !== undefined && !["queued", "running", "completed", "error", "skipped", "outdated"].includes(String(value.status))) fail();
    return itemList(await this.#transport.call("items.list", this.#instance, value));
  }
  async upload(name: string, content: ReadableStream | Blob | string, options?: AiSearchUploadItemOptions): Promise<AiSearchItemInfo> {
    const filename = text(name, 1024); const parsed = options === undefined ? {} : exact(options, ["metadata"]);
    const uploadOptions = parsed.metadata === undefined ? {} : { metadata: uploadMetadata(parsed.metadata) };
    let body: ReadableStream<Uint8Array>; let contentType: string;
    if (typeof content === "string") { const bytes = encoder.encode(content); if (bytes.byteLength > 4 * 1024 * 1024) fail("AI_SEARCH_LIMIT_EXCEEDED"); body = new Blob([bytes]).stream(); contentType = "text/plain"; }
    else if (content instanceof Blob) { if (content.size > 4 * 1024 * 1024) fail("AI_SEARCH_LIMIT_EXCEEDED"); body = content.stream(); contentType = content.type || "application/octet-stream"; }
    else if (content instanceof ReadableStream) { body = content as ReadableStream<Uint8Array>; contentType = "application/octet-stream"; }
    else fail();
    return itemInfo(await this.#transport.upload(this.#instance, filename, contentType, body, uploadOptions));
  }
  async uploadAndPoll(name: string, content: ReadableStream | Blob | string, options?: AiSearchUploadItemOptions & { pollIntervalMs?: number; timeoutMs?: number }): Promise<AiSearchItemInfo> {
    const raw = options === undefined ? {} : exact(options, ["metadata", "pollIntervalMs", "timeoutMs"]);
    const interval = raw.pollIntervalMs === undefined ? 1000 : integer(raw.pollIntervalMs, 10, 60_000);
    const timeout = raw.timeoutMs === undefined ? 30_000 : integer(raw.timeoutMs, interval, 300_000);
    let item = await this.upload(name, content, raw.metadata === undefined ? undefined : { metadata: raw.metadata as Record<string, string> });
    const deadline = Date.now() + timeout;
    while (["queued", "running", "outdated"].includes(item.status) && Date.now() < deadline) {
      await scheduler.wait(Math.min(interval, Math.max(0, deadline - Date.now()))); item = await this.get(item.id).info();
    }
    return item;
  }
  get(itemId: string): AiSearchItem { return new ItemBinding(this.#transport, this.#instance, itemId); }
  async delete(itemId: string): Promise<void> { if (await this.#transport.call("items.delete", this.#instance, { itemId: opaqueId(itemId) }) !== null) fail("AI_SEARCH_PROTOCOL_ERROR"); }
}
class JobBinding {
  readonly #transport: AiSearchTransport; readonly #instance: string | undefined; readonly #id: string;
  constructor(transport: AiSearchTransport, instance: string | undefined, id: string) { this.#transport = transport; this.#instance = instance; this.#id = opaqueId(id); }
  async info(): Promise<AiSearchJobInfo> { return jobInfo(await this.#transport.call("job.info", this.#instance, { jobId: this.#id })); }
  async logs(params?: AiSearchJobLogsParams): Promise<AiSearchJobLogsResponse> { return jobLogs(await this.#transport.call("job.logs", this.#instance, { jobId: this.#id, params: optionalPage(params, ["page", "per_page"]) })); }
  async cancel(): Promise<AiSearchJobInfo> { return jobInfo(await this.#transport.call("job.cancel", this.#instance, { jobId: this.#id })); }
}
class JobsBinding {
  readonly #transport: AiSearchTransport; readonly #instance: string | undefined;
  constructor(transport: AiSearchTransport, instance: string | undefined) { this.#transport = transport; this.#instance = instance; }
  async list(params?: AiSearchListJobsParams): Promise<AiSearchListJobsResponse> { return jobList(await this.#transport.call("jobs.list", this.#instance, optionalPage(params, ["page", "per_page"]))); }
  async create(params?: AiSearchCreateJobParams): Promise<AiSearchJobInfo> {
    const value = params === undefined ? {} : exact(params, ["description"]); if (value.description !== undefined) text(value.description, 4096);
    return jobInfo(await this.#transport.call("jobs.create", this.#instance, value));
  }
  get(jobId: string): AiSearchJob { return new JobBinding(this.#transport, this.#instance, jobId); }
}

/** Complete instance-level AI Search facade from the pinned declaration. */
export class AiSearchInstanceBinding {
  readonly #transport: AiSearchTransport; readonly #instance: string | undefined;
  constructor(raw: unknown, instance?: string | boolean) {
    if (!isTransport(raw)) fail("AI_SEARCH_UNAVAILABLE");
    this.#transport = raw;
    this.#instance = typeof instance === "string" ? instanceName(instance) : undefined;
  }
  async search(params: AiSearchSearchRequest): Promise<AiSearchSearchResponse> { return searchResponse(await this.#transport.call("instance.search", this.#instance, searchRequest(params, false)), false) as AiSearchSearchResponse; }
  chatCompletions(params: AiSearchChatCompletionsRequest & { stream: true }): Promise<ReadableStream>;
  chatCompletions(params: AiSearchChatCompletionsRequest): Promise<AiSearchChatCompletionsResponse>;
  chatCompletions(params: AiSearchChatCompletionsRequest): Promise<ReadableStream | AiSearchChatCompletionsResponse> {
    const value = chatRequest(params, false);
    return value.stream === true ? eventStream(this.#transport.stream("instance.chatCompletions", this.#instance, value))
      : this.#transport.call("instance.chatCompletions", this.#instance, value).then(result => chatResponse(result, false) as AiSearchChatCompletionsResponse);
  }
  async update(value: Partial<AiSearchConfig>): Promise<AiSearchInstanceInfo> { return instanceInfo(await this.#transport.call("instance.update", this.#instance, config(value, true))); }
  async info(): Promise<AiSearchInstanceInfo> { return instanceInfo(await this.#transport.call("instance.info", this.#instance, {})); }
  async stats(): Promise<AiSearchStatsResponse> { return stats(await this.#transport.call("instance.stats", this.#instance, {})); }
  get items(): AiSearchItems { return new ItemsBinding(this.#transport, this.#instance); }
  get jobs(): AiSearchJobs { return new JobsBinding(this.#transport, this.#instance); }
}

/** Complete namespace-level AI Search facade from the pinned declaration. */
export class AiSearchNamespaceBinding {
  readonly #transport: AiSearchTransport;
  constructor(raw: unknown) {
    if (!isTransport(raw)) fail("AI_SEARCH_UNAVAILABLE");
    this.#transport = raw;
  }
  get(name: string): AiSearchInstance { return new AiSearchInstanceBinding(this.#transport, instanceName(name)); }
  async list(params?: AiSearchListInstancesParams): Promise<AiSearchListResponse> { const value = optionalPage(params, ["page", "per_page", "search", "order_by", "order_by_direction"]); if (value.order_by !== undefined && value.order_by !== "created_at") fail(); if (value.order_by_direction !== undefined && !["asc", "desc"].includes(String(value.order_by_direction))) fail(); return instanceList(await this.#transport.call("namespace.list", undefined, value)); }
  async create(value: AiSearchConfig): Promise<AiSearchInstance> {
    const parsed = config(value, false); instanceInfo(await this.#transport.call("namespace.create", undefined, parsed));
    return new AiSearchInstanceBinding(this.#transport, parsed.id as string);
  }
  async delete(name: string): Promise<void> { if (await this.#transport.call("namespace.delete", undefined, { instance: instanceName(name) }) !== null) fail("AI_SEARCH_PROTOCOL_ERROR"); }
  async search(params: AiSearchMultiSearchRequest): Promise<AiSearchMultiSearchResponse> { return searchResponse(await this.#transport.call("namespace.search", undefined, searchRequest(params, true)), true) as AiSearchMultiSearchResponse; }
  chatCompletions(params: AiSearchMultiChatCompletionsRequest & { stream: true }): Promise<ReadableStream>;
  chatCompletions(params: AiSearchMultiChatCompletionsRequest): Promise<AiSearchMultiChatCompletionsResponse>;
  chatCompletions(params: AiSearchMultiChatCompletionsRequest): Promise<ReadableStream | AiSearchMultiChatCompletionsResponse> {
    const value = chatRequest(params, true);
    return value.stream === true ? eventStream(this.#transport.stream("namespace.chatCompletions", undefined, value))
      : this.#transport.call("namespace.chatCompletions", undefined, value).then(result => chatResponse(result, true) as AiSearchMultiChatCompletionsResponse);
  }
}
