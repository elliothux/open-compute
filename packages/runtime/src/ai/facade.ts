interface AiTransport {
  transform(files: WireDocument[], options: WireConversionOptions): Promise<unknown>;
  supported(): Promise<unknown>;
}

interface MarkdownDocument { name: string; blob: Blob }
interface WireDocument { name: string; mimeType: string; dataBase64: string }
type OutputFormat = "markdown" | "text";
interface WireConversionOptions {
  output?: { format?: OutputFormat };
  html?: { hostname?: string; cssSelector?: string };
  pdf?: { metadata?: boolean };
}
interface ConversionRequestOptions {
  gateway?: unknown;
  extraHeaders?: unknown;
  conversionOptions?: {
    output?: { format?: OutputFormat };
    html?: { images?: unknown; hostname?: string; cssSelector?: string };
    docx?: unknown;
    image?: unknown;
    pdf?: { images?: unknown; metadata?: boolean };
  };
}
interface SupportedFileFormat { extension: string; mimeType: string }
type ConversionResponse = {
  id: string; name: string; mimeType: string; format: OutputFormat; tokens: number; data: string;
} | { id: string; name: string; mimeType: string; format: "error"; error: string };

const MAX_FILE_BYTES = 4 * 1024 * 1024;
const MAX_BATCH_FILES = 16;
const MAX_BATCH_BYTES = 32 * 1024 * 1024;

function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function exact(value: unknown, fields: readonly string[], code = "AI_OPTION_UNSUPPORTED"): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some(key => !fields.includes(key))) throw new TypeError(code);
  return value;
}

function parseOptions(value: unknown): WireConversionOptions {
  if (value === undefined) return {};
  const request = exact(value, ["gateway", "extraHeaders", "conversionOptions"]);
  if (request.gateway !== undefined || request.extraHeaders !== undefined) throw new TypeError("AI_OPTION_UNSUPPORTED");
  if (request.conversionOptions === undefined) return {};
  const conversion = exact(request.conversionOptions, ["output", "html", "docx", "image", "pdf"]);
  if (conversion.docx !== undefined || conversion.image !== undefined) throw new TypeError("AI_OPTION_UNSUPPORTED");
  const result: WireConversionOptions = {};
  if (conversion.output !== undefined) {
    const output = exact(conversion.output, ["format"]);
    if (output.format !== undefined && output.format !== "markdown" && output.format !== "text") {
      throw new TypeError("AI_OPTION_UNSUPPORTED");
    }
    result.output = output.format === undefined ? {} : { format: output.format };
  }
  if (conversion.html !== undefined) {
    const html = exact(conversion.html, ["images", "hostname", "cssSelector"]);
    if (html.images !== undefined) throw new TypeError("AI_OPTION_UNSUPPORTED");
    if (html.hostname !== undefined) {
      if (typeof html.hostname !== "string" || html.hostname.length > 2048
          || /[\u0000-\u001f\u007f]/.test(html.hostname)) throw new TypeError("AI_OPTION_UNSUPPORTED");
      let base: URL;
      try {
        base = new URL(html.hostname.includes("://") ? html.hostname : `https://${html.hostname}`);
      } catch { throw new TypeError("AI_OPTION_UNSUPPORTED"); }
      if ((base.protocol !== "http:" && base.protocol !== "https:") || base.username || base.password) {
        throw new TypeError("AI_OPTION_UNSUPPORTED");
      }
    }
    if (html.cssSelector !== undefined
        && (typeof html.cssSelector !== "string" || !html.cssSelector.length
          || html.cssSelector.length > 512 || /[\u0000-\u001f\u007f{}@\\]/.test(html.cssSelector)
          || html.cssSelector.split(",").length > 16)) throw new TypeError("AI_OPTION_UNSUPPORTED");
    result.html = {
      ...(html.hostname === undefined ? {} : { hostname: html.hostname as string }),
      ...(html.cssSelector === undefined ? {} : { cssSelector: html.cssSelector as string }),
    };
  }
  if (conversion.pdf !== undefined) {
    const pdf = exact(conversion.pdf, ["images", "metadata"]);
    if (pdf.images !== undefined || (pdf.metadata !== undefined && typeof pdf.metadata !== "boolean")) {
      throw new TypeError("AI_OPTION_UNSUPPORTED");
    }
    result.pdf = pdf.metadata === undefined ? {} : { metadata: pdf.metadata };
  }
  return result;
}

function base64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.byteLength; offset += 16 * 1024) {
    binary += String.fromCharCode(...bytes.subarray(offset, Math.min(offset + 16 * 1024, bytes.byteLength)));
  }
  return btoa(binary);
}

async function wireDocument(value: unknown): Promise<WireDocument> {
  const file = exact(value, ["name", "blob"], "AI_DOCUMENT_INVALID");
  if (typeof file.name !== "string" || !file.name.length || file.name.length > 255
      || file.name === "." || file.name === ".." || /[\\/\u0000-\u001f\u007f]/.test(file.name)
      || !(file.blob instanceof Blob)
      || file.blob.size > MAX_FILE_BYTES || file.blob.size < 1
      || file.blob.type.length > 128
      || !/^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(file.blob.type)) {
    throw new TypeError(file.blob instanceof Blob && file.blob.size > MAX_FILE_BYTES
      ? "AI_DOCUMENT_TOO_LARGE" : "AI_DOCUMENT_INVALID");
  }
  return {
    name: file.name,
    mimeType: file.blob.type,
    dataBase64: base64(new Uint8Array(await file.blob.arrayBuffer())),
  };
}

function response(value: unknown, expectedName: string): ConversionResponse {
  if (!record(value) || typeof value.id !== "string" || !value.id.length || value.id.length > 256
      || value.name !== expectedName || typeof value.mimeType !== "string" || value.mimeType.length > 255) {
    throw new TypeError("AI_PROTOCOL_ERROR");
  }
  if (value.format === "error") {
    if (Object.keys(value).some(key => !["id", "name", "mimeType", "format", "error"].includes(key))
        || typeof value.error !== "string" || !value.error.length || value.error.length > 4096) {
      throw new TypeError("AI_PROTOCOL_ERROR");
    }
    return { id: value.id, name: value.name, mimeType: value.mimeType, format: "error", error: value.error };
  }
  if ((value.format !== "markdown" && value.format !== "text")
      || Object.keys(value).some(key => !["id", "name", "mimeType", "format", "tokens", "data"].includes(key))
      || typeof value.tokens !== "number" || !Number.isSafeInteger(value.tokens) || value.tokens < 0
      || typeof value.data !== "string") throw new TypeError("AI_PROTOCOL_ERROR");
  return {
    id: value.id, name: value.name, mimeType: value.mimeType, format: value.format,
    tokens: value.tokens, data: value.data,
  };
}

class ToMarkdownService {
  readonly #transport: AiTransport;
  constructor(transport: AiTransport) { this.#transport = transport; }
  async transform(files: MarkdownDocument[], options?: ConversionRequestOptions): Promise<ConversionResponse[]>;
  async transform(files: MarkdownDocument, options?: ConversionRequestOptions): Promise<ConversionResponse>;
  async transform(files: MarkdownDocument | MarkdownDocument[], options?: ConversionRequestOptions): Promise<ConversionResponse | ConversionResponse[]> {
    const many = Array.isArray(files);
    const input: unknown[] = many ? files : [files];
    const parsedOptions = parseOptions(options);
    if (input.length > MAX_BATCH_FILES) throw new TypeError("AI_BATCH_TOO_LARGE");
    let bytes = 0;
    for (const item of input) {
      if (record(item) && item.blob instanceof Blob) bytes += item.blob.size;
      if (bytes > MAX_BATCH_BYTES) throw new TypeError("AI_BATCH_TOO_LARGE");
    }
    const documents: WireDocument[] = [];
    for (const item of input) documents.push(await wireDocument(item));
    const raw = await this.#transport.transform(documents, parsedOptions);
    if (!Array.isArray(raw) || raw.length !== documents.length) throw new TypeError("AI_PROTOCOL_ERROR");
    const result = raw.map((item, index) => response(item, documents[index]!.name));
    return many ? result : result[0]!;
  }
  async supported(): Promise<SupportedFileFormat[]> {
    const raw = await this.#transport.supported();
    if (!Array.isArray(raw) || raw.length > 64) throw new TypeError("AI_PROTOCOL_ERROR");
    const result: SupportedFileFormat[] = [];
    const seen = new Set<string>();
    for (const value of raw) {
      if (!record(value) || Object.keys(value).some(key => !["extension", "mimeType"].includes(key))
          || typeof value.extension !== "string" || !/^\.[a-z0-9]{1,16}$/.test(value.extension)
          || typeof value.mimeType !== "string" || !/^[A-Za-z0-9!#$&^_.+-]+\/[A-Za-z0-9!#$&^_.+-]+$/.test(value.mimeType)) {
        throw new TypeError("AI_PROTOCOL_ERROR");
      }
      const key = `${value.extension}\0${value.mimeType}`;
      if (seen.has(key)) throw new TypeError("AI_PROTOCOL_ERROR");
      seen.add(key);
      result.push({ extension: value.extension, mimeType: value.mimeType });
    }
    const compare = (left: string, right: string) => left < right ? -1 : left > right ? 1 : 0;
    return result.sort((left, right) => compare(left.extension, right.extension)
      || compare(left.mimeType, right.mimeType));
  }
}

/** Cloudflare Workers AI facade intentionally limited to Markdown Conversion. */
export class AiBinding {
  readonly #service: ToMarkdownService;
  readonly aiGatewayLogId = null;
  constructor(raw: unknown) {
    if (!record(raw) || typeof Reflect.get(raw, "transform") !== "function"
        || typeof Reflect.get(raw, "supported") !== "function") throw new TypeError("AI_UNAVAILABLE");
    this.#service = new ToMarkdownService(raw as unknown as AiTransport);
  }
  toMarkdown(): ToMarkdownService;
  toMarkdown(files: MarkdownDocument[], options?: ConversionRequestOptions): Promise<ConversionResponse[]>;
  toMarkdown(files: MarkdownDocument, options?: ConversionRequestOptions): Promise<ConversionResponse>;
  toMarkdown(files?: MarkdownDocument | MarkdownDocument[], options?: ConversionRequestOptions): ToMarkdownService | Promise<ConversionResponse | ConversionResponse[]> {
    if (arguments.length === 0) return this.#service;
    if (Array.isArray(files)) return this.#service.transform(files, options);
    return this.#service.transform(files as MarkdownDocument, options);
  }
  async run(): Promise<never> { throw new TypeError("AI_UNSUPPORTED"); }
  async models(): Promise<never> { throw new TypeError("AI_UNSUPPORTED"); }
  gateway(): never { throw new TypeError("AI_UNSUPPORTED"); }
  aiSearch(): never { throw new TypeError("AI_UNSUPPORTED"); }
  autorag(): never { throw new TypeError("AI_UNSUPPORTED"); }
}
