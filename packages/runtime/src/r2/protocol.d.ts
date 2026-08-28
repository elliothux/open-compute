/** Product-supported HTTP metadata; backend JSON uses null for absent fields. */
export interface R2HttpMetadata {
  contentType?: string | null | undefined;
  contentLanguage?: string | null | undefined;
  contentDisposition?: string | null | undefined;
  contentEncoding?: string | null | undefined;
  cacheControl?: string | null | undefined;
  cacheExpiry?: number | null | undefined;
}
export interface R2Range {
  offset?: number | null;
  length?: number | null;
  suffix?: number | null;
}
export interface R2Condition {
  etagMatches: string[];
  etagDoesNotMatch: string[];
  uploadedBefore?: number | undefined;
  uploadedAfter?: number | undefined;
}
/** Version and checksum are absent in minimal list entries. */
export interface R2Metadata {
  key: string;
  version?: string;
  size: number;
  etag: string;
  httpEtag: string;
  uploaded: number;
  httpMetadata: R2HttpMetadata;
  customMetadata: Record<string, string>;
  range?: R2Range | null;
  md5?: string;
  storageClass: string;
}
export interface R2GetOptions {
  range?: R2Range | undefined;
  onlyIf?: R2Condition | undefined;
}
export interface R2PutOptions {
  onlyIf?: R2Condition | undefined;
  httpMetadata: R2HttpMetadata;
  customMetadata: Record<string, string>;
  md5?: string | number[] | undefined;
  storageClass: "Standard";
}
export interface R2ListOptions {
  prefix: string;
  delimiter?: string | undefined;
  cursor?: string | undefined;
  limit: number;
  include: string[];
}
export interface R2ListResult {
  objects: R2Metadata[];
  truncated: boolean;
  cursor: string | null;
  delimitedPrefixes: string[];
}
export interface R2RawTransport {
  head(key: string): Promise<R2Metadata | null>;
  get(key: string, options: R2GetOptions): Promise<{ meta: R2Metadata; body?: ReadableStream<Uint8Array> } | null>;
  put(key: string, body: ReadableStream<unknown>, options: R2PutOptions): Promise<R2Metadata | null>;
  delete(keys: string[]): Promise<void>;
  list(options: R2ListOptions): Promise<R2ListResult>;
}
