import {
  chatResponse,
  eventStream,
  instanceInfo,
  instanceList,
  itemChunks,
  itemInfo,
  itemList,
  itemLogs,
  jobInfo,
  jobList,
  jobLogs,
  searchResponse,
  stats,
} from "./responses.js";
import {
  chatRequest,
  config,
  encoder,
  exact,
  fail,
  instanceName,
  integer,
  opaqueId,
  optionalPage,
  searchRequest,
  text,
  uploadMetadata,
} from "./validation.js";

interface AiSearchTransport {
  call(
    operation: string,
    instance: string | undefined,
    payload: unknown,
  ): Promise<unknown>;
  stream(
    operation: string,
    instance: string | undefined,
    payload: unknown,
  ): Promise<Response>;
  upload(
    instance: string | undefined,
    name: string,
    contentType: string,
    body: ReadableStream<Uint8Array>,
    options: unknown,
  ): Promise<unknown>;
  download(instance: string | undefined, itemId: string): Promise<Response>;
}
function isTransport(value: unknown): value is AiSearchTransport {
  return (
    value !== null &&
    typeof value === "object" &&
    typeof Reflect.get(value, "call") === "function" &&
    typeof Reflect.get(value, "stream") === "function" &&
    typeof Reflect.get(value, "upload") === "function" &&
    typeof Reflect.get(value, "download") === "function"
  );
}

class ItemBinding {
  readonly #transport: AiSearchTransport;
  readonly #instance: string | undefined;
  readonly #id: string;
  constructor(
    transport: AiSearchTransport,
    instance: string | undefined,
    id: string,
  ) {
    this.#transport = transport;
    this.#instance = instance;
    this.#id = opaqueId(id);
  }
  async info(): Promise<AiSearchItemInfo> {
    return itemInfo(
      await this.#transport.call("item.info", this.#instance, {
        itemId: this.#id,
      }),
    );
  }
  async download(): Promise<AiSearchItemContentResult> {
    const response = await this.#transport.download(this.#instance, this.#id);
    const filename = response.headers.get("x-open-compute-filename");
    const contentType = response.headers.get("content-type");
    const size = Number(response.headers.get("content-length"));
    if (
      !response.ok ||
      response.body === null ||
      filename === null ||
      contentType === null ||
      !Number.isSafeInteger(size) ||
      size < 0
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
    return { body: response.body, filename, contentType, size };
  }
  async sync(): Promise<AiSearchItemInfo> {
    return itemInfo(
      await this.#transport.call("item.sync", this.#instance, {
        itemId: this.#id,
      }),
    );
  }
  async logs(
    params?: AiSearchItemLogsParams,
  ): Promise<AiSearchItemLogsResponse> {
    const value = optionalPage(params, ["limit", "cursor"]);
    if (value.limit !== undefined) integer(value.limit, 1, 100);
    return itemLogs(
      await this.#transport.call("item.logs", this.#instance, {
        itemId: this.#id,
        params: value,
      }),
    );
  }
  async chunks(
    params?: AiSearchItemChunksParams,
  ): Promise<AiSearchItemChunksResponse> {
    const value =
      params === undefined ? {} : exact(params, ["limit", "offset"]);
    if (value.limit !== undefined) integer(value.limit, 1, 100);
    if (value.offset !== undefined) integer(value.offset, 0, 1_000_000_000);
    return itemChunks(
      await this.#transport.call("item.chunks", this.#instance, {
        itemId: this.#id,
        params: value,
      }),
    );
  }
}
class ItemsBinding {
  readonly #transport: AiSearchTransport;
  readonly #instance: string | undefined;
  constructor(transport: AiSearchTransport, instance: string | undefined) {
    this.#transport = transport;
    this.#instance = instance;
  }
  async list(
    params?: AiSearchListItemsParams,
  ): Promise<AiSearchListItemsResponse> {
    const value = optionalPage(params, [
      "page",
      "per_page",
      "search",
      "sort_by",
      "status",
      "source",
      "metadata_filter",
      "item_id",
      "key",
    ]);
    if (
      value.sort_by !== undefined &&
      !["status", "modified_at"].includes(String(value.sort_by))
    )
      fail();
    if (
      value.status !== undefined &&
      ![
        "queued",
        "running",
        "completed",
        "error",
        "skipped",
        "outdated",
      ].includes(String(value.status))
    )
      fail();
    return itemList(
      await this.#transport.call("items.list", this.#instance, value),
    );
  }
  async upload(
    name: string,
    content: ReadableStream | Blob | string,
    options?: AiSearchUploadItemOptions,
  ): Promise<AiSearchItemInfo> {
    const filename = text(name, 1024);
    const parsed = options === undefined ? {} : exact(options, ["metadata"]);
    const uploadOptions =
      parsed.metadata === undefined
        ? {}
        : { metadata: uploadMetadata(parsed.metadata) };
    let body: ReadableStream<Uint8Array>;
    let contentType: string;
    if (typeof content === "string") {
      const bytes = encoder.encode(content);
      if (bytes.byteLength > 64 * 1024 * 1024) fail("AI_SEARCH_LIMIT_EXCEEDED");
      body = new Blob([bytes]).stream();
      contentType = "text/plain";
    } else if (content instanceof Blob) {
      if (content.size > 64 * 1024 * 1024) fail("AI_SEARCH_LIMIT_EXCEEDED");
      body = content.stream();
      contentType = content.type || "application/octet-stream";
    } else if (content instanceof ReadableStream) {
      body = content as ReadableStream<Uint8Array>;
      contentType = "application/octet-stream";
    } else fail();
    return itemInfo(
      await this.#transport.upload(
        this.#instance,
        filename,
        contentType,
        body,
        uploadOptions,
      ),
    );
  }
  async uploadAndPoll(
    name: string,
    content: ReadableStream | Blob | string,
    options?: AiSearchUploadItemOptions & {
      pollIntervalMs?: number;
      timeoutMs?: number;
    },
  ): Promise<AiSearchItemInfo> {
    const raw =
      options === undefined
        ? {}
        : exact(options, ["metadata", "pollIntervalMs", "timeoutMs"]);
    const interval =
      raw.pollIntervalMs === undefined
        ? 1000
        : integer(raw.pollIntervalMs, 10, 60_000);
    const timeout =
      raw.timeoutMs === undefined
        ? 30_000
        : integer(raw.timeoutMs, interval, 300_000);
    let item = await this.upload(
      name,
      content,
      raw.metadata === undefined
        ? undefined
        : { metadata: raw.metadata as Record<string, string> },
    );
    const deadline = Date.now() + timeout;
    while (
      ["queued", "running", "outdated"].includes(item.status) &&
      Date.now() < deadline
    ) {
      await scheduler.wait(
        Math.min(interval, Math.max(0, deadline - Date.now())),
      );
      item = await this.get(item.id).info();
    }
    return item;
  }
  get(itemId: string): AiSearchItem {
    return new ItemBinding(this.#transport, this.#instance, itemId);
  }
  async delete(itemId: string): Promise<void> {
    if (
      (await this.#transport.call("items.delete", this.#instance, {
        itemId: opaqueId(itemId),
      })) !== null
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  }
}
class JobBinding {
  readonly #transport: AiSearchTransport;
  readonly #instance: string | undefined;
  readonly #id: string;
  constructor(
    transport: AiSearchTransport,
    instance: string | undefined,
    id: string,
  ) {
    this.#transport = transport;
    this.#instance = instance;
    this.#id = opaqueId(id);
  }
  async info(): Promise<AiSearchJobInfo> {
    return jobInfo(
      await this.#transport.call("job.info", this.#instance, {
        jobId: this.#id,
      }),
    );
  }
  async logs(params?: AiSearchJobLogsParams): Promise<AiSearchJobLogsResponse> {
    return jobLogs(
      await this.#transport.call("job.logs", this.#instance, {
        jobId: this.#id,
        params: optionalPage(params, ["page", "per_page"]),
      }),
    );
  }
  async cancel(): Promise<AiSearchJobInfo> {
    return jobInfo(
      await this.#transport.call("job.cancel", this.#instance, {
        jobId: this.#id,
      }),
    );
  }
}
class JobsBinding {
  readonly #transport: AiSearchTransport;
  readonly #instance: string | undefined;
  constructor(transport: AiSearchTransport, instance: string | undefined) {
    this.#transport = transport;
    this.#instance = instance;
  }
  async list(
    params?: AiSearchListJobsParams,
  ): Promise<AiSearchListJobsResponse> {
    return jobList(
      await this.#transport.call(
        "jobs.list",
        this.#instance,
        optionalPage(params, ["page", "per_page"]),
      ),
    );
  }
  async create(params?: AiSearchCreateJobParams): Promise<AiSearchJobInfo> {
    const value = params === undefined ? {} : exact(params, ["description"]);
    if (value.description !== undefined) text(value.description, 4096);
    return jobInfo(
      await this.#transport.call("jobs.create", this.#instance, value),
    );
  }
  get(jobId: string): AiSearchJob {
    return new JobBinding(this.#transport, this.#instance, jobId);
  }
}

/** Complete instance-level AI Search facade from the pinned declaration. */
export class AiSearchInstanceBinding {
  readonly #transport: AiSearchTransport;
  readonly #instance: string | undefined;
  constructor(raw: unknown, instance?: string | boolean) {
    if (!isTransport(raw)) fail("AI_SEARCH_UNAVAILABLE");
    this.#transport = raw;
    this.#instance =
      typeof instance === "string" ? instanceName(instance) : undefined;
  }
  async search(params: AiSearchSearchRequest): Promise<AiSearchSearchResponse> {
    return searchResponse(
      await this.#transport.call(
        "instance.search",
        this.#instance,
        searchRequest(params, false),
      ),
      false,
    ) as AiSearchSearchResponse;
  }
  chatCompletions(
    params: AiSearchChatCompletionsRequest & { stream: true },
  ): Promise<ReadableStream>;
  chatCompletions(
    params: AiSearchChatCompletionsRequest,
  ): Promise<AiSearchChatCompletionsResponse>;
  chatCompletions(
    params: AiSearchChatCompletionsRequest,
  ): Promise<ReadableStream | AiSearchChatCompletionsResponse> {
    const value = chatRequest(params, false);
    return value.stream === true
      ? eventStream(
          this.#transport.stream(
            "instance.chatCompletions",
            this.#instance,
            value,
          ),
        )
      : this.#transport
          .call("instance.chatCompletions", this.#instance, value)
          .then(
            (result) =>
              chatResponse(result, false) as AiSearchChatCompletionsResponse,
          );
  }
  async update(value: Partial<AiSearchConfig>): Promise<AiSearchInstanceInfo> {
    return instanceInfo(
      await this.#transport.call(
        "instance.update",
        this.#instance,
        config(value, true),
      ),
    );
  }
  async info(): Promise<AiSearchInstanceInfo> {
    return instanceInfo(
      await this.#transport.call("instance.info", this.#instance, {}),
    );
  }
  async stats(): Promise<AiSearchStatsResponse> {
    return stats(
      await this.#transport.call("instance.stats", this.#instance, {}),
    );
  }
  get items(): AiSearchItems {
    return new ItemsBinding(this.#transport, this.#instance);
  }
  get jobs(): AiSearchJobs {
    return new JobsBinding(this.#transport, this.#instance);
  }
}

/** Complete namespace-level AI Search facade from the pinned declaration. */
export class AiSearchNamespaceBinding {
  readonly #transport: AiSearchTransport;
  constructor(raw: unknown) {
    if (!isTransport(raw)) fail("AI_SEARCH_UNAVAILABLE");
    this.#transport = raw;
  }
  get(name: string): AiSearchInstance {
    return new AiSearchInstanceBinding(this.#transport, instanceName(name));
  }
  async list(
    params?: AiSearchListInstancesParams,
  ): Promise<AiSearchListResponse> {
    const value = optionalPage(params, [
      "page",
      "per_page",
      "search",
      "order_by",
      "order_by_direction",
    ]);
    if (value.order_by !== undefined && value.order_by !== "created_at") fail();
    if (
      value.order_by_direction !== undefined &&
      !["asc", "desc"].includes(String(value.order_by_direction))
    )
      fail();
    return instanceList(
      await this.#transport.call("namespace.list", undefined, value),
    );
  }
  async create(value: AiSearchConfig): Promise<AiSearchInstance> {
    const parsed = config(value, false);
    instanceInfo(
      await this.#transport.call("namespace.create", undefined, parsed),
    );
    return new AiSearchInstanceBinding(this.#transport, parsed.id as string);
  }
  async delete(name: string): Promise<void> {
    if (
      (await this.#transport.call("namespace.delete", undefined, {
        instance: instanceName(name),
      })) !== null
    )
      fail("AI_SEARCH_PROTOCOL_ERROR");
  }
  async search(
    params: AiSearchMultiSearchRequest,
  ): Promise<AiSearchMultiSearchResponse> {
    return searchResponse(
      await this.#transport.call(
        "namespace.search",
        undefined,
        searchRequest(params, true),
      ),
      true,
    ) as AiSearchMultiSearchResponse;
  }
  chatCompletions(
    params: AiSearchMultiChatCompletionsRequest & { stream: true },
  ): Promise<ReadableStream>;
  chatCompletions(
    params: AiSearchMultiChatCompletionsRequest,
  ): Promise<AiSearchMultiChatCompletionsResponse>;
  chatCompletions(
    params: AiSearchMultiChatCompletionsRequest,
  ): Promise<ReadableStream | AiSearchMultiChatCompletionsResponse> {
    const value = chatRequest(params, true);
    return value.stream === true
      ? eventStream(
          this.#transport.stream("namespace.chatCompletions", undefined, value),
        )
      : this.#transport
          .call("namespace.chatCompletions", undefined, value)
          .then(
            (result) =>
              chatResponse(
                result,
                true,
              ) as AiSearchMultiChatCompletionsResponse,
          );
  }
}
