/** Day1 tenant types for the Cloudflare-compatible surface advertised by open-compute. */

type OpenComputeJson = null | boolean | number | string | OpenComputeJson[]
  | { [key: string]: OpenComputeJson };
type OpenComputeBodyInit = string | ArrayBuffer | ArrayBufferView | Blob
  | ReadableStream<Uint8Array> | URLSearchParams;
type OpenComputeRequestInfo = Request | string | URL;

declare class Headers implements Iterable<[string, string]> {
  constructor(init?: Headers | Record<string, string> | Iterable<readonly [string, string]>);
  append(name: string, value: string): void;
  delete(name: string): void;
  entries(): IterableIterator<[string, string]>;
  forEach(callback: (value: string, key: string, parent: Headers) => void): void;
  get(name: string): string | null;
  has(name: string): boolean;
  keys(): IterableIterator<string>;
  set(name: string, value: string): void;
  values(): IterableIterator<string>;
  [Symbol.iterator](): IterableIterator<[string, string]>;
}

interface OpenComputeBody {
  readonly body: ReadableStream<Uint8Array> | null;
  readonly bodyUsed: boolean;
  arrayBuffer(): Promise<ArrayBuffer>;
  blob(): Promise<Blob>;
  bytes(): Promise<Uint8Array>;
  formData(): Promise<FormData>;
  json<T = unknown>(): Promise<T>;
  text(): Promise<string>;
}

interface RequestInit {
  method?: string;
  headers?: Headers | Record<string, string> | Iterable<readonly [string, string]>;
  body?: OpenComputeBodyInit | null;
  redirect?: "follow" | "error" | "manual";
  signal?: AbortSignal | null;
}

declare class Request implements OpenComputeBody {
  constructor(input: OpenComputeRequestInfo, init?: RequestInit);
  readonly body: ReadableStream<Uint8Array> | null;
  readonly bodyUsed: boolean;
  readonly headers: Headers;
  readonly method: string;
  readonly redirect: string;
  readonly signal: AbortSignal;
  readonly url: string;
  arrayBuffer(): Promise<ArrayBuffer>;
  blob(): Promise<Blob>;
  bytes(): Promise<Uint8Array>;
  clone(): Request;
  formData(): Promise<FormData>;
  json<T = unknown>(): Promise<T>;
  text(): Promise<string>;
}

interface ResponseInit {
  status?: number;
  statusText?: string;
  headers?: Headers | Record<string, string> | Iterable<readonly [string, string]>;
}

declare class Response implements OpenComputeBody {
  constructor(body?: OpenComputeBodyInit | null, init?: ResponseInit);
  readonly body: ReadableStream<Uint8Array> | null;
  readonly bodyUsed: boolean;
  readonly headers: Headers;
  readonly ok: boolean;
  readonly redirected: boolean;
  readonly status: number;
  readonly statusText: string;
  readonly url: string;
  static error(): Response;
  static json(data: unknown, init?: ResponseInit): Response;
  static redirect(url: string | URL, status?: number): Response;
  arrayBuffer(): Promise<ArrayBuffer>;
  blob(): Promise<Blob>;
  bytes(): Promise<Uint8Array>;
  clone(): Response;
  formData(): Promise<FormData>;
  json<T = unknown>(): Promise<T>;
  text(): Promise<string>;
}

declare class URL {
  constructor(url: string | URL, base?: string | URL);
  hash: string;
  host: string;
  hostname: string;
  href: string;
  origin: string;
  password: string;
  pathname: string;
  port: string;
  protocol: string;
  readonly searchParams: URLSearchParams;
  search: string;
  username: string;
  toString(): string;
}

declare class URLSearchParams implements Iterable<[string, string]> {
  constructor(init?: string | Record<string, string> | Iterable<readonly [string, string]>);
  append(name: string, value: string): void;
  delete(name: string, value?: string): void;
  get(name: string): string | null;
  getAll(name: string): string[];
  has(name: string, value?: string): boolean;
  set(name: string, value: string): void;
  sort(): void;
  toString(): string;
  [Symbol.iterator](): IterableIterator<[string, string]>;
}

declare class Blob {
  constructor(parts?: readonly unknown[], options?: { type?: string });
  readonly size: number;
  readonly type: string;
  arrayBuffer(): Promise<ArrayBuffer>;
  bytes(): Promise<Uint8Array>;
  slice(start?: number, end?: number, contentType?: string): Blob;
  stream(): ReadableStream<Uint8Array>;
  text(): Promise<string>;
}

interface FormData extends Iterable<[string, string | Blob]> {}
interface AbortSignal { readonly aborted: boolean; readonly reason: unknown }

interface ReadableStreamReadResult<T> { done: boolean; value?: T }
interface ReadableStreamDefaultReader<T> {
  readonly closed: Promise<void>;
  cancel(reason?: unknown): Promise<void>;
  read(): Promise<ReadableStreamReadResult<T>>;
  releaseLock(): void;
}
declare class ReadableStream<T = unknown> {
  constructor(source?: unknown, strategy?: unknown);
  readonly locked: boolean;
  cancel(reason?: unknown): Promise<void>;
  getReader(): ReadableStreamDefaultReader<T>;
  pipeThrough<R>(transform: { readable: ReadableStream<R>; writable: WritableStream<T> }): ReadableStream<R>;
  pipeTo(destination: WritableStream<T>): Promise<void>;
  tee(): [ReadableStream<T>, ReadableStream<T>];
}
declare class WritableStream<T = unknown> { constructor(sink?: unknown, strategy?: unknown) }
declare class TransformStream<I = unknown, O = unknown> {
  constructor(transformer?: unknown, writableStrategy?: unknown, readableStrategy?: unknown);
  readonly readable: ReadableStream<O>;
  readonly writable: WritableStream<I>;
}

declare class WebSocket {
  constructor(url: string | URL, protocols?: string | string[]);
  readonly readyState: number;
  accept(): void;
  close(code?: number, reason?: string): void;
  send(data: string | ArrayBuffer | ArrayBufferView): void;
}
declare class WebSocketPair { readonly 0: WebSocket; readonly 1: WebSocket }

declare function fetch(input: OpenComputeRequestInfo, init?: RequestInit): Promise<Response>;

interface ExecutionContext<Props = unknown> {
  readonly props: Props;
  waitUntil(promise: Promise<unknown>): void;
  passThroughOnException(): void;
}
interface ScheduledController { readonly scheduledTime: number; readonly cron: string; noRetry(): void }

interface ExportedHandler<Env = unknown, QueueBody = unknown, Props = unknown> {
  fetch?(request: Request, env: Env, ctx: ExecutionContext<Props>): Response | Promise<Response>;
  scheduled?(controller: ScheduledController, env: Env, ctx: ExecutionContext<Props>): void | Promise<void>;
  queue?(batch: MessageBatch<QueueBody>, env: Env, ctx: ExecutionContext<Props>): void | Promise<void>;
}

interface Fetcher {
  fetch(input: OpenComputeRequestInfo, init?: RequestInit): Promise<Response>;
}

interface KVNamespaceListKey<Metadata = unknown> { name: string; expiration?: number; metadata?: Metadata }
interface KVNamespaceListResult<Metadata = unknown> {
  list_complete: boolean;
  keys: KVNamespaceListKey<Metadata>[];
  cursor?: string;
}
interface KVNamespace<Metadata = unknown> {
  get(key: string, type?: "text"): Promise<string | null>;
  get(key: string, type: "arrayBuffer"): Promise<ArrayBuffer | null>;
  get<T = unknown>(key: string, type: "json"): Promise<T | null>;
  get(key: string, type: "stream"): Promise<ReadableStream<Uint8Array> | null>;
  getWithMetadata<T = string>(key: string, options?: unknown): Promise<{ value: T | null; metadata: Metadata | null }>;
  put(key: string, value: string | ArrayBuffer | ArrayBufferView | ReadableStream<Uint8Array>,
    options?: { expiration?: number; expirationTtl?: number; metadata?: Metadata }): Promise<void>;
  delete(key: string): Promise<void>;
  list(options?: { prefix?: string; limit?: number; cursor?: string }): Promise<KVNamespaceListResult<Metadata>>;
}

interface R2Range { offset?: number; length?: number; suffix?: number }
interface R2Conditional { etagMatches?: string | string[]; etagDoesNotMatch?: string | string[]; uploadedBefore?: Date; uploadedAfter?: Date }
interface R2HttpMetadata { contentType?: string; contentLanguage?: string; contentDisposition?: string; contentEncoding?: string; cacheControl?: string; cacheExpiry?: Date }
interface R2Object {
  readonly key: string; readonly version?: string; readonly size: number; readonly etag: string;
  readonly httpEtag: string; readonly uploaded: Date; readonly httpMetadata: R2HttpMetadata;
  readonly customMetadata: Record<string, string>; readonly range?: R2Range | null;
  readonly checksums: { md5?: string }; readonly storageClass: "Standard";
  writeHttpMetadata(headers: Headers): void;
}
interface R2ObjectBody extends R2Object, OpenComputeBody {}
interface R2Bucket {
  head(key: string): Promise<R2Object | null>;
  get(key: string, options?: { range?: R2Range | Headers; onlyIf?: R2Conditional | Headers }): Promise<R2ObjectBody | R2Object | null>;
  put(key: string, value: OpenComputeBodyInit | null, options?: { onlyIf?: R2Conditional | Headers; httpMetadata?: R2HttpMetadata | Headers; customMetadata?: Record<string, string>; md5?: string | ArrayBuffer | ArrayBufferView; storageClass?: "Standard" }): Promise<R2Object | null>;
  delete(keys: string | string[]): Promise<void>;
  list(options?: { prefix?: string; delimiter?: string; cursor?: string; limit?: number; include?: ("httpMetadata" | "customMetadata")[] }): Promise<{ objects: R2Object[]; truncated: boolean; cursor?: string; delimitedPrefixes: string[] }>;
}

interface D1Result<T = Record<string, unknown>> { results: T[]; success: boolean; meta: Record<string, unknown>; error?: string }
interface D1PreparedStatement {
  bind(...values: unknown[]): D1PreparedStatement;
  first<T = Record<string, unknown>>(column?: string): Promise<T | null>;
  run<T = Record<string, unknown>>(): Promise<D1Result<T>>;
  all<T = Record<string, unknown>>(): Promise<D1Result<T>>;
  raw<T extends unknown[] = unknown[]>(options?: { columnNames?: boolean }): Promise<T[]>;
}
interface D1DatabaseSession {
  prepare(query: string): D1PreparedStatement;
  batch<T = Record<string, unknown>>(statements: D1PreparedStatement[]): Promise<D1Result<T>[]>;
}
interface D1Database extends D1DatabaseSession {
  exec(query: string): Promise<{ count: number; duration: number }>;
  withSession(constraint?: "first-primary" | "first-unconstrained"): D1DatabaseSession;
}

interface DurableObjectId { readonly name?: string; toString(): string; equals(other: DurableObjectId): boolean }
interface DurableObjectStub extends Fetcher { readonly id: DurableObjectId; [method: string]: unknown }
interface DurableObjectNamespace {
  idFromName(name: string): DurableObjectId;
  newUniqueId(): DurableObjectId;
  idFromString(id: string): DurableObjectId;
  get(id: DurableObjectId): DurableObjectStub;
  getByName(name: string): DurableObjectStub;
}
interface DurableObjectStorage {
  get<T = unknown>(key: string): Promise<T | undefined>;
  put<T>(key: string, value: T): Promise<void>;
  delete(key: string): Promise<boolean>;
  list<T = unknown>(options?: { start?: string; startAfter?: string; end?: string; prefix?: string; reverse?: boolean; limit?: number }): Promise<Map<string, T>>;
  transaction<T>(closure: (transaction: DurableObjectStorage) => Promise<T>): Promise<T>;
  getAlarm(): Promise<number | null>;
  setAlarm(scheduledTime: number | Date): Promise<void>;
  deleteAlarm(): Promise<void>;
}
interface DurableObjectState { readonly id: DurableObjectId; readonly storage: DurableObjectStorage; waitUntil(promise: Promise<unknown>): void }

interface QueueSendOptions { contentType?: "json" | "text" | "bytes"; delaySeconds?: number }
interface Queue<Body = unknown> {
  send(body: Body, options?: QueueSendOptions): Promise<void>;
  sendBatch(messages: Iterable<{ body: Body; contentType?: "json" | "text" | "bytes"; delaySeconds?: number }>): Promise<void>;
  metrics(): Promise<{ messages: number; bytes: number }>;
}
interface Message<Body = unknown> {
  readonly id: string; readonly timestamp: Date; readonly body: Body; readonly attempts: number;
  ack(): void; retry(options?: { delaySeconds?: number }): void;
}
interface MessageBatch<Body = unknown> {
  readonly queue: string; readonly messages: readonly Message<Body>[];
  ackAll(): void; retryAll(options?: { delaySeconds?: number }): void;
}

interface WorkflowStatus { status: string; output?: OpenComputeJson; error?: string }
interface WorkflowInstance {
  readonly id: string; status(): Promise<WorkflowStatus>; sendEvent(event: { type: string; payload?: OpenComputeJson }): Promise<void>;
  pause(): Promise<void>; resume(): Promise<void>; terminate(): Promise<void>; restart(): Promise<void>;
}
interface Workflow { create(options?: { id?: string; params?: OpenComputeJson }): Promise<WorkflowInstance>; get(id: string): WorkflowInstance }
interface WorkflowStep {
  do<T extends OpenComputeJson>(name: string, callback: () => Promise<T>): Promise<T>;
  sleep(name: string, duration: string | number): Promise<void>;
  sleepUntil(name: string, timestamp: Date | number): Promise<void>;
  waitForEvent<T extends OpenComputeJson>(name: string, options: { type: string; timeout?: string | number }): Promise<T>;
}
interface WorkflowEvent<Payload extends OpenComputeJson = OpenComputeJson> {
  readonly payload: Payload;
  readonly timestamp: Date;
  readonly instanceId: string;
  readonly workflowName: string;
}

interface OpenComputeCache {
  put(request: OpenComputeRequestInfo, response: Response): Promise<void>;
  match(request: OpenComputeRequestInfo, options?: { ignoreMethod?: boolean }): Promise<Response | undefined>;
  delete(request: OpenComputeRequestInfo, options?: { ignoreMethod?: boolean }): Promise<boolean>;
}
interface OpenComputeCacheStorage { readonly default: OpenComputeCache; open(name: string): Promise<OpenComputeCache> }
declare const caches: OpenComputeCacheStorage;

interface ImagesBinding {
  input(stream: ReadableStream<Uint8Array>): ImageTransformer;
  info(stream: ReadableStream<Uint8Array>): Promise<{ format: "jpeg" | "png" | "webp"; fileSize: number; width: number; height: number }>;
}
interface ImageTransformer {
  transform(options: { width?: number; height?: number; fit?: "scale-down" | "contain" | "cover" | "crop" | "pad"; rotate?: 90 | 180 | 270; flip?: "h" | "v" | "hv"; background?: string; blur?: number }): this;
  draw(stream: ReadableStream<Uint8Array>, options?: { left?: number; top?: number; opacity?: number; repeat?: false; composite?: "normal" | "over" }): this;
  output(options: { format: "image/jpeg" | "image/png" | "image/webp" | "image/avif"; quality?: number; anim?: false }): Promise<ImageTransformationResult>;
}
interface ImageTransformationResult { response(options?: { headers?: Headers | Record<string, string> }): Response; contentType(): string; image(): ReadableStream<Uint8Array> }
interface VersionMetadata { readonly id: string; readonly tag: string | null; readonly timestamp: string }
interface OpenComputeExecutionContext extends ExecutionContext { readonly cache: { purge(options: { tags?: string[]; prefixes?: string[] }): Promise<{ success: boolean; deleted: number }> } }

declare module "cloudflare:workers" {
  export class RpcTarget {}
  export class WorkerEntrypoint<Env = unknown, Props = unknown> extends RpcTarget {
    readonly env: Env; readonly ctx: ExecutionContext<Props>;
  }
  export class DurableObject<Env = unknown> extends RpcTarget {
    readonly ctx: DurableObjectState; readonly env: Env;
  }
  export class WorkflowEntrypoint<Env = unknown, Payload extends OpenComputeJson = OpenComputeJson>
    extends RpcTarget {
    readonly env: Env; readonly ctx: ExecutionContext;
    run(event: WorkflowEvent<Payload>, step: WorkflowStep): unknown | Promise<unknown>;
  }
}

declare module "cloudflare:workflows" {
  export class NonRetryableError extends Error {}
}
