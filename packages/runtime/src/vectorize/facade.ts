interface VectorizeTransport {
  call(operation: string, payload: unknown): Promise<unknown>;
  mutate(operation: "insert" | "upsert", frame: ReadableStream<Uint8Array>): Promise<unknown>;
}
function isTransport(value: unknown): value is VectorizeTransport {
  return value !== null && typeof value === "object" && typeof Reflect.get(value, "call") === "function"
    && typeof Reflect.get(value, "mutate") === "function";
}

const encoder = new TextEncoder();
const ID_BYTES = 64;
const METADATA_BYTES = 10 * 1024;
const MAX_DIMENSIONS = 1536;
const MAX_BATCH = 1000;

function fail(code = "VECTORIZE_INPUT_INVALID"): never { throw new TypeError(code); }
function record(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}
function exact(value: unknown, keys: readonly string[]): Record<string, unknown> {
  if (!record(value) || Object.keys(value).some(key => !keys.includes(key))) fail();
  return value;
}
function boundedString(value: unknown, maximum = ID_BYTES): string {
  if (typeof value !== "string" || value.length === 0 || /[\u0000-\u001f\u007f]/.test(value)
      || encoder.encode(value).byteLength > maximum) fail();
  return value;
}
function finiteNumber(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value)) fail();
  return value;
}
function vectorValues(value: unknown): number[] {
  if (!Array.isArray(value) && !(value instanceof Float32Array) && !(value instanceof Float64Array)) fail();
  if (value.length < 1 || value.length > MAX_DIMENSIONS) fail();
  return Array.from(value, item => {
    const number = finiteNumber(item);
    if (!Number.isFinite(Math.fround(number))) fail();
    return Math.fround(number);
  });
}
function metadataValue(value: unknown, nested = false): unknown {
  if (typeof value === "string" || typeof value === "boolean") return value;
  if (typeof value === "number") return finiteNumber(value);
  if (Array.isArray(value) && value.every(item => typeof item === "string")) return [...value];
  if (!nested && record(value)) {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, metadataValue(item, true)]));
  }
  fail();
}
function metadata(value: unknown): Record<string, VectorizeVectorMetadata> {
  if (!record(value)) fail();
  const result = Object.fromEntries(Object.entries(value).map(([key, item]) => [key, metadataValue(item)]));
  if (encoder.encode(JSON.stringify(result)).byteLength > METADATA_BYTES) fail("VECTORIZE_LIMIT_EXCEEDED");
  return result as Record<string, VectorizeVectorMetadata>;
}
function vector(value: unknown): VectorizeVector {
  const item = exact(value, ["id", "values", "namespace", "metadata"]);
  return {
    id: boundedString(item.id), values: vectorValues(item.values),
    ...(item.namespace === undefined ? {} : { namespace: boundedString(item.namespace) }),
    ...(item.metadata === undefined ? {} : { metadata: metadata(item.metadata) }),
  };
}
function batch(value: unknown): VectorizeVector[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > MAX_BATCH) fail("VECTORIZE_LIMIT_EXCEEDED");
  return value.map(vector);
}
function ids(value: unknown): string[] {
  if (!Array.isArray(value) || value.length < 1 || value.length > MAX_BATCH) fail("VECTORIZE_LIMIT_EXCEEDED");
  return value.map(item => boundedString(item));
}
function filter(value: unknown): VectorizeVectorMetadataFilter {
  if (!record(value) || Object.keys(value).length < 1 || Object.keys(value).length > 64) fail();
  const output: Record<string, unknown> = {};
  for (const [field, raw] of Object.entries(value)) {
    boundedString(field, 256);
    if (raw === null || typeof raw === "string" || typeof raw === "boolean" || typeof raw === "number") {
      output[field] = raw === null ? null : typeof raw === "number" ? finiteNumber(raw) : raw;
      continue;
    }
    const operators = exact(raw, ["$eq", "$ne", "$lt", "$lte", "$gt", "$gte", "$in", "$nin"]);
    const entries = Object.entries(operators);
    if (entries.length < 1 || entries.length > 2) fail();
    const names = entries.map(([name]) => name);
    if (entries.length === 2 && !(names.some(name => name === "$gt" || name === "$gte")
        && names.some(name => name === "$lt" || name === "$lte"))) fail();
    const parsed: Record<string, unknown> = {};
    for (const [operator, operand] of entries) {
      if (operator === "$in" || operator === "$nin") {
        if (entries.length !== 1 || !Array.isArray(operand) || operand.length === 0 || operand.length > 100
            || !operand.every(item => typeof item === "string" || typeof item === "boolean"
              || (typeof item === "number" && Number.isFinite(item)))) fail();
        parsed[operator] = operand;
      } else if (["$lt", "$lte", "$gt", "$gte"].includes(operator)) {
        if (typeof operand !== "string" && (typeof operand !== "number" || !Number.isFinite(operand))) fail();
        parsed[operator] = operand;
      } else if (operand === null || typeof operand === "string" || typeof operand === "boolean"
          || (typeof operand === "number" && Number.isFinite(operand))) parsed[operator] = operand;
      else fail();
    }
    output[field] = parsed;
  }
  if (encoder.encode(JSON.stringify(output)).byteLength > 2048) fail("VECTORIZE_LIMIT_EXCEEDED");
  return output as VectorizeVectorMetadataFilter;
}
function queryOptions(value: unknown): VectorizeQueryOptions {
  if (value === undefined) return {};
  const options = exact(value, ["topK", "namespace", "returnValues", "returnMetadata", "filter"]);
  const topK = options.topK ?? 5;
  if (!Number.isSafeInteger(topK) || (topK as number) < 1 || (topK as number) > 100) fail("VECTORIZE_LIMIT_EXCEEDED");
  if (options.returnValues !== undefined && typeof options.returnValues !== "boolean") fail();
  if (options.returnMetadata !== undefined && typeof options.returnMetadata !== "boolean"
      && !["all", "indexed", "none"].includes(String(options.returnMetadata))) fail();
  if ((options.returnValues === true || options.returnMetadata === true || options.returnMetadata === "all")
      && (topK as number) > 50) fail("VECTORIZE_LIMIT_EXCEEDED");
  return {
    topK: topK as number,
    ...(options.namespace === undefined ? {} : { namespace: boundedString(options.namespace) }),
    ...(options.returnValues === undefined ? {} : { returnValues: options.returnValues as boolean }),
    ...(options.returnMetadata === undefined ? {} : {
      returnMetadata: options.returnMetadata === true ? "all"
        : options.returnMetadata === false ? "none" : options.returnMetadata as VectorizeMetadataRetrievalLevel,
    }),
    ...(options.filter === undefined ? {} : { filter: filter(options.filter) }),
  };
}

function mutationFrame(operation: "insert" | "upsert", vectors: VectorizeVector[]): ReadableStream<Uint8Array> {
  const chunks: Uint8Array[] = [];
  let size = 0;
  const push = (bytes: Uint8Array): void => { chunks.push(bytes); size += bytes.byteLength; };
  const number = (bytes: number, value: number): void => {
    const raw = new Uint8Array(bytes); const view = new DataView(raw.buffer);
    if (bytes === 2) view.setUint16(0, value, false); else view.setUint32(0, value, false);
    push(raw);
  };
  const header = encoder.encode(JSON.stringify({ operation, schemaVersion: 1 }));
  push(encoder.encode("OCVZ")); number(2, 1); number(4, header.byteLength); push(header); number(4, vectors.length);
  for (const item of vectors) {
    const id = encoder.encode(item.id); number(2, id.byteLength); push(id);
    const namespace = item.namespace === undefined ? undefined : encoder.encode(item.namespace);
    number(2, namespace?.byteLength ?? 0xffff); if (namespace) push(namespace);
    const meta = item.metadata === undefined ? undefined : encoder.encode(JSON.stringify(item.metadata));
    number(4, meta?.byteLength ?? 0xffffffff); if (meta) push(meta);
    const values = vectorValues(item.values); number(2, values.length);
    const raw = new Uint8Array(values.length * 4); const view = new DataView(raw.buffer);
    values.forEach((value, index) => view.setFloat32(index * 4, value, true)); push(raw);
  }
  const output = new Uint8Array(size); let offset = 0;
  for (const chunk of chunks) { output.set(chunk, offset); offset += chunk.byteLength; }
  return new ReadableStream({ start(controller) { controller.enqueue(output); controller.close(); } });
}
function mutation(value: unknown): VectorizeAsyncMutation {
  const result = exact(value, ["mutationId"]);
  return { mutationId: boundedString(result.mutationId, 128) };
}
function described(value: unknown): VectorizeIndexInfo {
  const result = exact(value, ["vectorCount", "dimensions", "processedUpToDatetime", "processedUpToMutation"]);
  for (const key of ["vectorCount", "dimensions", "processedUpToDatetime", "processedUpToMutation"]) {
    if (!Number.isSafeInteger(result[key]) || (result[key] as number) < 0) fail("VECTORIZE_PROTOCOL_ERROR");
  }
  return {
    vectorCount: result.vectorCount as number,
    dimensions: result.dimensions as number,
    processedUpToDatetime: result.processedUpToDatetime as number,
    processedUpToMutation: result.processedUpToMutation as number,
  };
}
function matches(value: unknown): VectorizeMatches {
  const result = exact(value, ["matches", "count"]);
  if (!Array.isArray(result.matches) || !Number.isSafeInteger(result.count) || result.count !== result.matches.length) fail("VECTORIZE_PROTOCOL_ERROR");
  const parsed = result.matches.map(raw => {
    const item = exact(raw, ["id", "values", "namespace", "metadata", "score"]);
    const output: VectorizeMatch = {
      id: boundedString(item.id), score: finiteNumber(item.score),
      ...(item.values === undefined ? {} : { values: vectorValues(item.values) }),
      ...(item.namespace === undefined ? {} : { namespace: boundedString(item.namespace) }),
      ...(item.metadata === undefined ? {} : { metadata: metadata(item.metadata) }),
    };
    return output;
  });
  return { matches: parsed, count: result.count as number };
}

/** Latest stable Vectorize API over an immutable resource transport. */
export class VectorizeBinding {
  readonly #transport: VectorizeTransport;
  constructor(raw: unknown) {
    if (!isTransport(raw)) fail("VECTORIZE_UNAVAILABLE");
    this.#transport = raw;
  }
  async describe(): Promise<VectorizeIndexInfo> { return described(await this.#transport.call("describe", {})); }
  async query(input: VectorFloatArray | number[], options?: VectorizeQueryOptions): Promise<VectorizeMatches> {
    return matches(await this.#transport.call("query", { vector: vectorValues(input), options: queryOptions(options) }));
  }
  async queryById(id: string, options?: VectorizeQueryOptions): Promise<VectorizeMatches> {
    return matches(await this.#transport.call("queryById", { vectorId: boundedString(id), options: queryOptions(options) }));
  }
  async insert(vectors: VectorizeVector[]): Promise<VectorizeAsyncMutation> {
    const value = batch(vectors); return mutation(await this.#transport.mutate("insert", mutationFrame("insert", value)));
  }
  async upsert(vectors: VectorizeVector[]): Promise<VectorizeAsyncMutation> {
    const value = batch(vectors); return mutation(await this.#transport.mutate("upsert", mutationFrame("upsert", value)));
  }
  async deleteByIds(value: string[]): Promise<VectorizeAsyncMutation> {
    return mutation(await this.#transport.call("deleteByIds", { ids: ids(value) }));
  }
  async getByIds(value: string[]): Promise<VectorizeVector[]> {
    const result = await this.#transport.call("getByIds", { ids: ids(value) });
    if (!Array.isArray(result)) fail("VECTORIZE_PROTOCOL_ERROR");
    return result.map(vector);
  }
}
