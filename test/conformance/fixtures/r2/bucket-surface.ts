interface Env {
  BUCKET: R2Bucket;
}

export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const key = "object";
    const body = "content";
    const headers = new Headers({
      "content-type": "text/plain",
      "if-match": "\"etag\"",
      range: "bytes=0-1",
      expires: new Date(0).toUTCString(),
    });
    const putPlain: R2Object | null = await env.BUCKET.put(key, body);
    const putNull: R2Object | null = await env.BUCKET.put(key, body, {
      onlyIf: { etagMatches: "etag", uploadedAfter: new Date(0), secondsGranularity: true },
      httpMetadata: { contentType: "text/plain", cacheExpiry: new Date(1) },
      customMetadata: { a: "b" },
      md5: "5d41402abc4b2a76b9719d911017c592",
      storageClass: "Standard",
    });
    const putIa: R2Object = await env.BUCKET.put(key, new Uint8Array(), {
      sha1: new ArrayBuffer(20),
      storageClass: "InfrequentAccess",
    });
    const putSha256: R2Object = await env.BUCKET.put(key, new ArrayBuffer(0), { sha256: "00".repeat(32) });
    const putSha384: R2Object = await env.BUCKET.put(key, null, { sha384: new Uint8Array(48) });
    const putSha512: R2Object = await env.BUCKET.put(key, new Blob(), { sha512: "00".repeat(64), httpMetadata: headers });
    const putSsec: R2Object = await env.BUCKET.put(key, body, {
      ssecKey: "00".repeat(32),
    });
    const head: R2Object | null = await env.BUCKET.head(key);
    const got: R2ObjectBody | null = await env.BUCKET.get(key);
    const conditional: R2ObjectBody | R2Object | null = await env.BUCKET.get(key, {
      onlyIf: headers,
      range: { offset: 0, length: 1 },
      ssecKey: new ArrayBuffer(32),
    });
    const ranged: R2ObjectBody | null = await env.BUCKET.get(key, { range: { suffix: 1 } });
    await env.BUCKET.delete(key);
    await env.BUCKET.delete([key, "other"]);
    const listed: R2Objects = await env.BUCKET.list({
      prefix: "p",
      delimiter: "/",
      cursor: "c",
      startAfter: "s",
      limit: 10,
      include: ["httpMetadata", "customMetadata"],
    });
    const objects: R2Object[] = listed.objects;
    const prefixes: string[] = listed.delimitedPrefixes;
    if (listed.truncated) {
      const cursor: string = listed.cursor;
      void cursor;
    } else {
      // @ts-expect-error complete pages have no cursor
      const _missing: string = listed.cursor;
      void _missing;
    }
    const created: R2MultipartUpload = await env.BUCKET.createMultipartUpload(key, {
      httpMetadata: { contentLanguage: "en" },
      customMetadata: { k: "v" },
      storageClass: "InfrequentAccess",
      ssecKey: "11".repeat(32),
    });
    const resumed: R2MultipartUpload = env.BUCKET.resumeMultipartUpload(created.key, created.uploadId);
    const uploaded: R2UploadedPart = await resumed.uploadPart(1, body, { ssecKey: "11".repeat(32) });
    const completed: R2Object = await resumed.complete([uploaded]);
    await created.abort();
    const checksums: R2Checksums = putIa.checksums;
    const json: R2StringChecksums = checksums.toJSON();
    const md5: ArrayBuffer | undefined = checksums.md5;
    const version: string = putIa.version;
    const ssecMd5: string | undefined = putSsec.ssecKeyMd5;
    const storage: string = putIa.storageClass;
    head?.writeHttpMetadata(new Headers());
    const text: string = got ? await got.text() : "";
    const bytes: Uint8Array = got ? await (await env.BUCKET.get(key))!.bytes() : new Uint8Array();
    const buf: ArrayBuffer = got ? await (await env.BUCKET.get(key))!.arrayBuffer() : new ArrayBuffer(0);
    const blob: Blob = got ? await (await env.BUCKET.get(key))!.blob() : new Blob();
    const parsed: unknown = got ? await (await env.BUCKET.get(key))!.json() : null;
    const used: boolean = got ? got.bodyUsed : false;
    const stream: ReadableStream | null = got ? got.body : null;
    void putNull;
    void putSha256;
    void putSha384;
    void putSha512;
    void conditional;
    void ranged;
    void objects;
    void prefixes;
    void completed;
    void json;
    void md5;
    void version;
    void ssecMd5;
    void storage;
    void text;
    void bytes;
    void buf;
    void blob;
    void parsed;
    void used;
    void stream;
    return new Response("ok");
  },
};

function negativeTypes(env: Env): void {
  // @ts-expect-error include only accepts httpMetadata | customMetadata
  void env.BUCKET.list({ include: ["etag"] });
  // @ts-expect-error etagMatches is a string, not an array
  void env.BUCKET.put("k", "v", { onlyIf: { etagMatches: ["a"] } });
  // @ts-expect-error head does not take get options
  void env.BUCKET.head("k", { range: { offset: 0 } });
}
