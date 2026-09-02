interface ImageTransport {
  input(stream: ReadableStream<Uint8Array>): Promise<string>;
  info(stream: ReadableStream<Uint8Array>): Promise<ImageInfo>;
  transform(session: string, options: TransportTransformOptions): Promise<void>;
  draw(session: string, stream: ReadableStream<Uint8Array>, options: TransportDrawOptions): Promise<void>;
  output(session: string, options: TransportOutputOptions): Promise<Response>;
}
interface ImageInfo { format: "jpeg" | "png" | "webp"; fileSize: number; width: number; height: number }
interface TransformOptions {
  width?: number; height?: number; fit?: "scale-down" | "contain" | "cover" | "crop" | "pad";
  gravity?: "center" | "top" | "bottom" | "left" | "right" | "top-left" | "top-right" | "bottom-left" | "bottom-right";
  rotate?: 90 | 180 | 270; flip?: "h" | "v" | "hv";
  background?: string; blur?: number;
}
interface TransportTransformOptions extends Omit<TransformOptions, "flip"> {
  flip?: "horizontal" | "vertical" | "both";
}
interface DrawOptions { left?: number; top?: number; opacity?: number; repeat?: false; composite?: "normal" | "over" }
interface TransportDrawOptions { left?: number; top?: number; opacity?: number; repeat?: false; blend?: "normal" | "over" }
type OutputFormat = "image/jpeg" | "image/png" | "image/webp" | "image/avif";
interface OutputOptions { format: OutputFormat; quality?: number; anim?: false }
interface TransportOutputOptions { format: "jpeg" | "png" | "webp" | "avif"; quality?: number; anim?: false }

function stream(value: unknown): ReadableStream<Uint8Array> {
  if (!(value instanceof ReadableStream)) throw new TypeError("IMAGE_INPUT_INVALID");
  return value as ReadableStream<Uint8Array>;
}

function known(value: unknown, fields: readonly string[]): Record<string, unknown> {
  if (value === null || typeof value !== "object" || Array.isArray(value)) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  const object = value as Record<string, unknown>;
  if (Object.keys(object).some(key => !fields.includes(key))) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  return object;
}

function transformOptions(value: unknown): TransportTransformOptions {
  const options = known(value, ["width", "height", "fit", "gravity", "rotate", "flip", "background", "blur"]);
  const flip = options.flip;
  if (flip !== undefined && !["h", "v", "hv"].includes(String(flip))) {
    throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  }
  return {
    ...(options.width === undefined ? {} : { width: options.width as number }),
    ...(options.height === undefined ? {} : { height: options.height as number }),
    ...(options.fit === undefined ? {} : { fit: options.fit as NonNullable<TransformOptions["fit"]> }),
    ...(options.gravity === undefined ? {} : {
      gravity: options.gravity as NonNullable<TransformOptions["gravity"]>,
    }),
    ...(options.rotate === undefined ? {} : {
      rotate: options.rotate as NonNullable<TransformOptions["rotate"]>,
    }),
    ...(flip === undefined ? {} : {
      flip: flip === "h" ? "horizontal" : flip === "v" ? "vertical" : "both",
    }),
    ...(options.background === undefined ? {} : { background: options.background as string }),
    ...(options.blur === undefined ? {} : { blur: options.blur as number }),
  };
}

function drawOptions(value: unknown): TransportDrawOptions {
  const options = known(value, ["left", "top", "opacity", "repeat", "composite"]);
  if (options.repeat !== undefined && options.repeat !== false) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  if (options.composite !== undefined && !["normal", "over"].includes(String(options.composite))) {
    throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  }
  return {
    ...(options.left === undefined ? {} : { left: options.left as number }),
    ...(options.top === undefined ? {} : { top: options.top as number }),
    ...(options.opacity === undefined ? {} : { opacity: options.opacity as number }),
    ...(options.repeat === undefined ? {} : { repeat: false as const }),
    ...(options.composite === undefined ? {} : {
      blend: options.composite as NonNullable<TransportDrawOptions["blend"]>,
    }),
  };
}

function outputOptions(value: unknown): TransportOutputOptions {
  const options = known(value, ["format", "quality", "anim"]);
  const format = options.format;
  if (!["image/jpeg", "image/png", "image/webp", "image/avif"].includes(String(format))
      || (options.anim !== undefined && options.anim !== false)) {
    throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
  }
  const backend = String(format).slice("image/".length);
  return {
    format: backend as TransportOutputOptions["format"],
    ...(options.quality === undefined ? {} : { quality: options.quality as number }),
    ...(options.anim === undefined ? {} : { anim: false as const }),
  };
}

class TransformationResult {
  readonly #response: Response;
  readonly #contentType: OutputFormat;
  constructor(response: Response, contentType: OutputFormat) {
    if (!(response instanceof Response) || response.status !== 200 || response.body === null
        || response.headers.get("content-type") !== contentType) {
      throw new TypeError("IMAGE_PROTOCOL_ERROR");
    }
    this.#response = response;
    this.#contentType = contentType;
  }
  response(options?: { headers?: HeadersInit }): Response {
    if (options !== undefined) known(options, ["headers"]);
    const headers = new Headers(this.#response.headers);
    if (options?.headers !== undefined) {
      new Headers(options.headers).forEach((value, name) => headers.set(name, value));
    }
    return new Response(this.#response.body, {
      status: this.#response.status,
      statusText: this.#response.statusText,
      headers,
    });
  }
  contentType(): string { return this.#contentType; }
  image(options?: { encoding?: "base64" }): ReadableStream<Uint8Array> {
    if (options !== undefined) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
    if (!this.#response.body) throw new TypeError("IMAGE_UNAVAILABLE");
    return this.#response.body;
  }
}

class Transformer {
  readonly #transport: ImageTransport;
  readonly #session: Promise<string>;
  #pending: Promise<void>;
  constructor(transport: ImageTransport, input: ReadableStream<Uint8Array>) {
    this.#transport = transport;
    this.#session = transport.input(input);
    this.#pending = this.#session.then(() => undefined);
  }
  transform(value: TransformOptions): this {
    const options = transformOptions(value);
    this.#pending = this.#pending.then(async () => {
      await this.#transport.transform(await this.#session, options);
    });
    return this;
  }
  draw(value: ReadableStream<Uint8Array>, options: DrawOptions = {}): this {
    const input = stream(value);
    const parsed = drawOptions(options);
    this.#pending = this.#pending.then(async () => {
      await this.#transport.draw(await this.#session, input, parsed);
    });
    return this;
  }
  async output(value: OutputOptions): Promise<TransformationResult> {
    const options = outputOptions(value);
    await this.#pending;
    return new TransformationResult(
      await this.#transport.output(await this.#session, options),
      value.format,
    );
  }
}

/** Strict public Images chain over one version-scoped native transport. */
export class ImagesBinding {
  readonly #transport: ImageTransport;
  constructor(raw: unknown) {
    if (raw === null || typeof raw !== "object" || typeof Reflect.get(raw, "input") !== "function"
        || typeof Reflect.get(raw, "info") !== "function") throw new TypeError("IMAGE_UNAVAILABLE");
    this.#transport = raw as ImageTransport;
  }
  input(value: ReadableStream<Uint8Array>, options?: unknown): Transformer {
    if (options !== undefined) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
    return new Transformer(this.#transport, stream(value));
  }
  async info(value: ReadableStream<Uint8Array>, options?: unknown): Promise<ImageInfo> {
    if (options !== undefined) throw new TypeError("IMAGE_OPTION_UNSUPPORTED");
    const result: unknown = await this.#transport.info(stream(value));
    if (result === null || typeof result !== "object" || Array.isArray(result)) {
      throw new TypeError("IMAGE_PROTOCOL_ERROR");
    }
    const info = result as Record<string, unknown>;
    const { format, fileSize, width, height } = info;
    if (Object.keys(info).some(key => !["format", "fileSize", "width", "height"].includes(key))
        || (format !== "jpeg" && format !== "png" && format !== "webp")
        || typeof fileSize !== "number" || !Number.isSafeInteger(fileSize) || fileSize < 1
        || typeof width !== "number" || !Number.isSafeInteger(width) || width < 1
        || typeof height !== "number" || !Number.isSafeInteger(height) || height < 1) {
      throw new TypeError("IMAGE_PROTOCOL_ERROR");
    }
    return { format, fileSize, width, height };
  }
}
