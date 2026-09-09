import {
  encoder,
  fail,
  instanceName,
  integer,
  number,
  opaqueId,
  protocolExact,
  record,
  text,
} from "./validation.js";

function json(value: unknown, depth = 0): boolean {
  if (depth > 16) return false;
  if (value === null || typeof value === "string" || typeof value === "boolean")
    return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value))
    return (
      value.length <= 10_000 && value.every((item) => json(item, depth + 1))
    );
  return (
    record(value) &&
    Object.keys(value).length <= 1_000 &&
    Object.values(value).every((item) => json(item, depth + 1))
  );
}
function pagination(value: unknown): void {
  if (value === undefined) return;
  const info = protocolExact(value, [
    "count",
    "page",
    "per_page",
    "total_count",
  ]);
  for (const key of ["count", "page", "per_page", "total_count"])
    integer(info[key], key === "page" ? 1 : 0, 1_000_000_000);
}
export function instanceInfo(value: unknown): AiSearchInstanceInfo {
  const info = protocolExact(value, [
    "id",
    "type",
    "source",
    "source_params",
    "paused",
    "status",
    "namespace",
    "created_at",
    "modified_at",
    "token_id",
    "ai_gateway_id",
    "rewrite_query",
    "reranking",
    "embedding_model",
    "ai_search_model",
    "rewrite_model",
    "reranking_model",
    "hybrid_search_enabled",
    "index_method",
    "fusion_method",
    "indexing_options",
    "retrieval_options",
    "chunk",
    "chunk_size",
    "chunk_overlap",
    "score_threshold",
    "max_num_results",
    "cache",
    "cache_threshold",
    "custom_metadata",
    "sync_interval",
    "metadata",
  ]);
  instanceName(info.id);
  for (const key of [
    "type",
    "source",
    "status",
    "namespace",
    "created_at",
    "modified_at",
    "token_id",
    "ai_gateway_id",
    "embedding_model",
    "ai_search_model",
    "rewrite_model",
    "reranking_model",
  ])
    if (info[key] !== undefined && typeof info[key] !== "string")
      fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of [
    "paused",
    "rewrite_query",
    "reranking",
    "hybrid_search_enabled",
    "chunk",
    "cache",
  ])
    if (info[key] !== undefined && typeof info[key] !== "boolean")
      fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of [
    "chunk_size",
    "chunk_overlap",
    "score_threshold",
    "max_num_results",
    "sync_interval",
  ])
    if (
      info[key] !== undefined &&
      (typeof info[key] !== "number" || !Number.isFinite(info[key]))
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of [
    "source_params",
    "index_method",
    "indexing_options",
    "retrieval_options",
    "custom_metadata",
    "metadata",
  ])
    if (info[key] !== undefined && !json(info[key]))
      fail("AI_SEARCH_PROTOCOL_ERROR");
  return info as AiSearchInstanceInfo;
}
export function itemInfo(value: unknown): AiSearchItemInfo {
  const info = protocolExact(value, [
    "id",
    "key",
    "status",
    "next_action",
    "error",
    "checksum",
    "namespace",
    "chunks_count",
    "file_size",
    "source_id",
    "last_seen_at",
    "created_at",
    "metadata",
  ]);
  opaqueId(info.id);
  text(info.key, 1024);
  if (
    ![
      "completed",
      "error",
      "skipped",
      "queued",
      "running",
      "outdated",
    ].includes(String(info.status))
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (
    info.next_action !== undefined &&
    info.next_action !== null &&
    !["INDEX", "DELETE"].includes(String(info.next_action))
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of [
    "error",
    "checksum",
    "namespace",
    "source_id",
    "last_seen_at",
    "created_at",
  ])
    if (
      info[key] !== undefined &&
      info[key] !== null &&
      typeof info[key] !== "string"
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of ["chunks_count", "file_size"])
    if (
      info[key] !== undefined &&
      info[key] !== null &&
      (!Number.isSafeInteger(info[key]) || (info[key] as number) < 0)
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  if (info.metadata !== undefined && !json(info.metadata))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  return info as AiSearchItemInfo;
}
function chunk(
  value: unknown,
  multi: boolean,
): AiSearchSearchResponse["chunks"][number] {
  const raw = protocolExact(value, [
    "id",
    "type",
    "score",
    "text",
    "item",
    "scoring_details",
    ...(multi ? ["instance_id"] : []),
  ]);
  opaqueId(raw.id);
  text(raw.type, 128);
  number(raw.score, 0, 1);
  if (
    typeof raw.text !== "string" ||
    encoder.encode(raw.text).byteLength > 1024 * 1024
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  const item = protocolExact(raw.item, ["timestamp", "key", "metadata"]);
  text(item.key, 1024);
  if (
    item.timestamp !== undefined &&
    (typeof item.timestamp !== "number" || !Number.isFinite(item.timestamp))
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (item.metadata !== undefined && !json(item.metadata))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (raw.scoring_details !== undefined && !json(raw.scoring_details))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (multi && typeof raw.instance_id !== "string")
    fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchSearchResponse["chunks"][number];
}
export function searchResponse(
  value: unknown,
  multi: boolean,
): AiSearchSearchResponse | AiSearchMultiSearchResponse {
  const raw = protocolExact(value, [
    "search_query",
    "chunks",
    ...(multi ? ["errors"] : []),
  ]);
  if (
    typeof raw.search_query !== "string" ||
    !Array.isArray(raw.chunks) ||
    raw.chunks.length > 50
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  raw.chunks.map((item) => chunk(item, multi));
  if (multi && raw.errors !== undefined) {
    if (!Array.isArray(raw.errors) || raw.errors.length > 10)
      fail("AI_SEARCH_PROTOCOL_ERROR");
    for (const error of raw.errors) {
      const item = protocolExact(error, ["instance_id", "message"]);
      instanceName(item.instance_id);
      text(item.message, 4096);
    }
  }
  return raw as AiSearchSearchResponse | AiSearchMultiSearchResponse;
}
export function chatResponse(
  value: unknown,
  multi: boolean,
): AiSearchChatCompletionsResponse | AiSearchMultiChatCompletionsResponse {
  const raw = protocolExact(value, [
    "id",
    "object",
    "model",
    "choices",
    "chunks",
    ...(multi ? ["errors"] : []),
  ]);
  for (const key of ["id", "object", "model"])
    if (raw[key] !== undefined && typeof raw[key] !== "string")
      fail("AI_SEARCH_PROTOCOL_ERROR");
  if (
    !Array.isArray(raw.choices) ||
    raw.choices.length > 100 ||
    !Array.isArray(raw.chunks) ||
    raw.chunks.length > 50
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const choice of raw.choices) {
    const item = protocolExact(choice, ["index", "message"]);
    if (item.index !== undefined) integer(item.index, 0, 1000);
    const message = protocolExact(item.message, ["role", "content"]);
    if (
      !["system", "developer", "user", "assistant", "tool"].includes(
        String(message.role),
      ) ||
      (message.content !== null && typeof message.content !== "string")
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  raw.chunks.map((item) => chunk(item, multi));
  if (multi && raw.errors !== undefined) {
    if (!Array.isArray(raw.errors) || raw.errors.length > 10)
      fail("AI_SEARCH_PROTOCOL_ERROR");
    for (const error of raw.errors) {
      const item = protocolExact(error, ["instance_id", "message"]);
      instanceName(item.instance_id);
      text(item.message, 4096);
    }
  }
  return raw as
    AiSearchChatCompletionsResponse | AiSearchMultiChatCompletionsResponse;
}
export function stats(value: unknown): AiSearchStatsResponse {
  const raw = protocolExact(value, [
    "queued",
    "running",
    "completed",
    "error",
    "skipped",
    "outdated",
    "last_activity",
    "engine",
  ]);
  for (const key of [
    "queued",
    "running",
    "completed",
    "error",
    "skipped",
    "outdated",
  ])
    if (raw[key] !== undefined) integer(raw[key], 0, 1_000_000_000);
  if (raw.last_activity !== undefined && typeof raw.last_activity !== "string")
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (raw.engine !== undefined && !json(raw.engine))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchStatsResponse;
}
export function itemLogs(value: unknown): AiSearchItemLogsResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) {
    const item = protocolExact(entry, [
      "timestamp",
      "action",
      "message",
      "fileKey",
      "chunkCount",
      "processingTimeMs",
      "errorType",
    ]);
    text(item.timestamp);
    text(item.action);
    if (typeof item.message !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  const info = protocolExact(raw.result_info, [
    "count",
    "per_page",
    "cursor",
    "truncated",
  ]);
  integer(info.count, 0, 1_000_000_000);
  integer(info.per_page, 0, 100);
  if (info.cursor !== null && typeof info.cursor !== "string")
    fail("AI_SEARCH_PROTOCOL_ERROR");
  if (typeof info.truncated !== "boolean") fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchItemLogsResponse;
}
export function itemChunks(value: unknown): AiSearchItemChunksResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) {
    const item = protocolExact(entry, [
      "id",
      "text",
      "start_byte",
      "end_byte",
      "item",
    ]);
    opaqueId(item.id);
    if (typeof item.text !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
    integer(item.start_byte, 0, 1_000_000_000);
    integer(item.end_byte, 0, 1_000_000_000);
    const source = protocolExact(item.item, ["timestamp", "key", "metadata"]);
    text(source.key, 1024);
    if (
      source.timestamp !== undefined &&
      (typeof source.timestamp !== "number" ||
        !Number.isFinite(source.timestamp))
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
    if (source.metadata !== undefined && !json(source.metadata))
      fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  const info = protocolExact(raw.result_info, [
    "count",
    "total",
    "limit",
    "offset",
  ]);
  for (const key of ["count", "total", "limit", "offset"])
    integer(info[key], 0, 1_000_000_000);
  return raw as AiSearchItemChunksResponse;
}
export function jobInfo(value: unknown): AiSearchJobInfo {
  const raw = protocolExact(value, [
    "id",
    "source",
    "description",
    "last_seen_at",
    "started_at",
    "ended_at",
    "end_reason",
  ]);
  opaqueId(raw.id);
  if (!["user", "schedule"].includes(String(raw.source)))
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const key of [
    "description",
    "last_seen_at",
    "started_at",
    "ended_at",
    "end_reason",
  ])
    if (
      raw[key] !== undefined &&
      raw[key] !== null &&
      typeof raw[key] !== "string"
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  return raw as AiSearchJobInfo;
}
export function jobLogs(value: unknown): AiSearchJobLogsResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  for (const entry of raw.result) {
    const item = protocolExact(entry, [
      "id",
      "message",
      "message_type",
      "created_at",
    ]);
    integer(item.id, 0, 1_000_000_000);
    if (typeof item.message !== "string") fail("AI_SEARCH_PROTOCOL_ERROR");
    integer(item.message_type, 0, 1_000_000);
    if (
      typeof item.created_at !== "number" ||
      !Number.isFinite(item.created_at)
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  pagination(raw.result_info);
  return raw as AiSearchJobLogsResponse;
}
export function instanceList(value: unknown): AiSearchListResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  raw.result.map(instanceInfo);
  pagination(raw.result_info);
  return raw as AiSearchListResponse;
}
export function itemList(value: unknown): AiSearchListItemsResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  raw.result.map(itemInfo);
  pagination(raw.result_info);
  return raw as AiSearchListItemsResponse;
}
export function jobList(value: unknown): AiSearchListJobsResponse {
  const raw = protocolExact(value, ["result", "result_info"]);
  if (!Array.isArray(raw.result) || raw.result.length > 100)
    fail("AI_SEARCH_PROTOCOL_ERROR");
  raw.result.map(jobInfo);
  pagination(raw.result_info);
  return raw as AiSearchListJobsResponse;
}
export async function eventStream(
  response: Promise<Response>,
): Promise<ReadableStream> {
  const value = await response;
  if (
    !(value instanceof Response) ||
    value.status !== 200 ||
    value.body === null ||
    value.headers
      .get("content-type")
      ?.split(";", 1)[0]
      ?.trim()
      .toLowerCase() !== "text/event-stream"
  )
    fail("AI_SEARCH_PROTOCOL_ERROR");
  return value.body;
}
