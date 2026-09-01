/** HTTP metadata. Backend JSON uses null/omission for absent fields. */
export interface R2HttpMetadata {
  contentType?: string | null;
  contentLanguage?: string | null;
  contentDisposition?: string | null;
  contentEncoding?: string | null;
  cacheControl?: string | null;
  cacheExpiry?: number | null;
}
export interface R2Range {
  offset?: number | null;
  length?: number | null;
  suffix?: number | null;
}
export type R2EtagMatch =
  | { kind: "wildcard" }
  | { kind: "strong"; value: string }
  | { kind: "weak"; value: string };
export interface R2Condition {
  etagMatches: R2EtagMatch[];
  etagDoesNotMatch: R2EtagMatch[];
  uploadedBefore?: number;
  uploadedAfter?: number;
  secondsGranularity?: boolean;
  httpHeaders?: boolean;
}
export interface R2Checksums {
  md5?: string;
  sha1?: string;
  sha256?: string;
  sha384?: string;
  sha512?: string;
}
export interface R2Metadata {
  key: string;
  version: string;
  size: number;
  etag: string;
  httpEtag: string;
  uploaded: number;
  httpMetadata?: R2HttpMetadata | null;
  customMetadata?: Record<string, string> | null;
  range?: R2Range | null;
  checksums: R2Checksums;
  storageClass: string;
  ssecKeyMd5?: string | null;
}
export interface R2GetOptions {
  range?: R2Range;
  onlyIf?: R2Condition;
  ssecKey?: string;
}
export interface R2PutOptions {
  onlyIf?: R2Condition;
  httpMetadata: R2HttpMetadata;
  customMetadata: Record<string, string>;
  checksum?: { algorithm: "md5" | "sha1" | "sha256" | "sha384" | "sha512"; hex: string };
  storageClass?: string;
  ssecKey?: string;
}
export interface R2MultipartCreateOptions {
  httpMetadata: R2HttpMetadata;
  customMetadata: Record<string, string>;
  storageClass?: string;
  ssecKey?: string;
}
export interface R2ListOptions {
  prefix: string;
  delimiter?: string;
  cursor?: string;
  startAfter?: string;
  limit: number;
  include: string[];
}
export interface R2ListResult {
  objects: R2Metadata[];
  truncated: boolean;
  cursor?: string;
  delimitedPrefixes: string[];
}
export interface R2UploadedPart {
  partNumber: number;
  etag: string;
}
export interface R2RawTransport {
  head(key: string): Promise<R2Metadata | null>;
  get(key: string, options: R2GetOptions): Promise<{ meta: R2Metadata; body?: ReadableStream<Uint8Array> } | null>;
  put(key: string, body: ReadableStream<unknown>, options: R2PutOptions): Promise<R2Metadata | null>;
  delete(keys: string[]): Promise<void>;
  list(options: R2ListOptions): Promise<R2ListResult>;
  createMultipartUpload(key: string, options: R2MultipartCreateOptions): Promise<{ key: string; uploadId: string }>;
  uploadPart(key: string, uploadId: string, partNumber: number, body: ReadableStream<unknown>, ssecKey?: string): Promise<R2UploadedPart>;
  completeMultipartUpload(key: string, uploadId: string, parts: R2UploadedPart[]): Promise<R2Metadata>;
  abortMultipartUpload(key: string, uploadId: string): Promise<void>;
}
