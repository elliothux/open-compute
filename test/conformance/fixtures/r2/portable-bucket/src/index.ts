interface Env {
  BUCKET: R2Bucket;
}

interface ErrorObservation {
  synchronous: boolean;
  name: string;
  message: string;
}

function invoke(method: Function, owner: object, args: unknown[]): unknown {
  return Reflect.apply(method, owner, args);
}

async function capture(call: () => unknown): Promise<ErrorObservation | null> {
  let synchronous = true;
  try {
    const pending = call();
    synchronous = false;
    await pending;
    return null;
  } catch (error) {
    return {
      synchronous,
      name: error instanceof Error ? error.name : typeof error,
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

async function clear(bucket: R2Bucket): Promise<void> {
  await bucket.delete(["", ".", "..", "a/../b", "k".repeat(1024)]);
  let cursor: string | undefined;
  do {
    const page = await bucket.list({
      prefix: "portable:",
      ...(cursor === undefined ? {} : { cursor }),
    });
    if (page.objects.length > 0) await bucket.delete(page.objects.map(object => object.key));
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor !== undefined);
}

async function text(bucket: R2Bucket, key: string): Promise<string | null> {
  const object = await bucket.get(key);
  return object === null ? null : object.text();
}

async function reset(bucket: R2Bucket): Promise<Response> {
  await clear(bucket);
  return Response.json({ reset: (await bucket.list({ prefix: "portable:" })).objects.length === 0 });
}

async function surface(bucket: R2Bucket): Promise<Response> {
  const put = await bucket.put("portable:a", "hello", {
    httpMetadata: {
      contentType: "text/plain",
      contentLanguage: "en",
      contentDisposition: "inline",
      contentEncoding: "identity",
      cacheControl: "max-age=60",
      cacheExpiry: new Date(0),
    },
    customMetadata: { tag: "primary" },
    md5: "5d41402abc4b2a76b9719d911017c592",
    storageClass: "Standard",
  });
  await bucket.put("portable:dir/one", "one");
  await bucket.put("portable:dir/two", "two");
  await bucket.put("portable:json", JSON.stringify({ ok: true }), {
    httpMetadata: { contentType: "application/json" },
  });
  const stream = new Response("stream-value").body!;
  await bucket.put("portable:stream", stream);
  await bucket.put("portable:z", new Uint8Array([1, 2, 3]));
  const checksumAlgorithms = {
    sha1: (await bucket.put("portable:sha1", "x", {
      sha1: "11f6ad8ec52a2984abaafd7c3b516503785c2072",
    })).checksums.toJSON(),
    sha256: (await bucket.put("portable:sha256", "x", {
      sha256: "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881",
    })).checksums.toJSON(),
    sha384: (await bucket.put("portable:sha384", "x", {
      sha384: "d752c2c51fba0e29aa190570a9d4253e44077a058d3297fa3a5630d5bd012622f97c28acaed313b5c83bb990caa7da85",
    })).checksums.toJSON(),
    sha512: (await bucket.put("portable:sha512", "x", {
      sha512: "a4abd4448c49562d828115d13a1fccea927f52b4d5459297f8b43e42da89238bc13626e43dcb38ddb082488927ec904fb42057443983e88585179d50551afe62",
    })).checksums.toJSON(),
  };
  const exactKey = "k".repeat(1024);
  for (const key of ["", ".", "..", "a/../b", exactKey]) await bucket.put(key, "key");
  const keyShapes = (await Promise.all(
    ["", ".", "..", "a/../b", exactKey].map(async key => (await bucket.head(key))?.key === key),
  )).every(Boolean);

  const head = await bucket.head("portable:a");
  const conditional = await bucket.get("portable:a", { onlyIf: { etagMatches: put.etag } });
  const notModified = await bucket.get("portable:a", { onlyIf: { etagDoesNotMatch: put.etag } });
  const failedPut = await bucket.put("portable:a", "changed", { onlyIf: { etagMatches: "missing" } });
  const ranged = await bucket.get("portable:a", { range: { offset: 1, length: 3 } });
  const suffix = await bucket.get("portable:a", { range: { suffix: 2 } });
  const bytes = await (await bucket.get("portable:z"))!.bytes();
  const arrayBuffer = await (await bucket.get("portable:z"))!.arrayBuffer();
  const blob = await (await bucket.get("portable:a"))!.blob();
  const json = await (await bucket.get("portable:json"))!.json<{ ok: boolean }>();
  const consumed = await bucket.get("portable:a");
  const beforeUsed = consumed!.bodyUsed;
  await consumed!.text();
  const headers = new Headers();
  head!.writeHttpMetadata(headers);
  const delimited = await bucket.list({
    prefix: "portable:",
    delimiter: "/",
    include: ["httpMetadata", "customMetadata"],
  });
  const after = await bucket.list({ prefix: "portable:", startAfter: "portable:a", limit: 2 });
  await bucket.put("portable:delete-a", "a");
  await bucket.put("portable:delete-b", "b");
  await bucket.delete(["portable:delete-a", "portable:delete-b"]);

  return Response.json({
    object: {
      key: put.key,
      size: put.size,
      etag: put.etag === "5d41402abc4b2a76b9719d911017c592",
      httpEtag: put.httpEtag === `"${put.etag}"`,
      version: typeof put.version === "string" && put.version.length > 0,
      uploaded: put.uploaded instanceof Date && Number.isFinite(put.uploaded.getTime()),
      checksums: put.checksums.toJSON(),
      storageClass: put.storageClass,
      ssecAbsent: put.ssecKeyMd5 === undefined,
    },
    head: {
      sameEtag: head?.etag === put.etag,
      contentType: head?.httpMetadata?.contentType,
      customTag: head?.customMetadata?.tag,
    },
    headers: Object.fromEntries(headers),
    bodies: {
      conditional: conditional === null || !("body" in conditional) ? null : await conditional.text(),
      notModifiedHasBody: notModified !== null && "body" in notModified,
      failedPut,
      ranged: ranged === null || !("body" in ranged) ? null : {
        text: await ranged.text(),
        range: ranged.range,
      },
      suffix: suffix === null ? null : await suffix.text(),
      bytes: [...bytes],
      arrayBuffer: [...new Uint8Array(arrayBuffer)],
      blob: { text: await blob.text(), type: blob.type },
      json,
      bodyUsed: [beforeUsed, consumed!.bodyUsed],
      stream: await text(bucket, "portable:stream"),
    },
    checksumAlgorithms,
    keyShapes,
    list: {
      objects: delimited.objects.map(object => ({
        key: object.key,
        hasHttpMetadata: Object.hasOwn(object, "httpMetadata"),
        hasCustomMetadata: Object.hasOwn(object, "customMetadata"),
      })),
      prefixes: delimited.delimitedPrefixes,
      truncated: delimited.truncated,
      after: after.objects.map(object => object.key),
    },
    deleted: await bucket.head("portable:delete-a") === null
      && await bucket.head("portable:delete-b") === null,
  });
}

async function multipart(bucket: R2Bucket): Promise<Response> {
  const upload = await bucket.createMultipartUpload("portable:multipart", {
    httpMetadata: { contentType: "text/plain" },
    customMetadata: { tag: "multipart" },
    storageClass: "InfrequentAccess",
  });
  const resumed = bucket.resumeMultipartUpload(upload.key, upload.uploadId);
  const part = await resumed.uploadPart(1, "hello");
  const completed = await resumed.complete([part]);
  const body = await bucket.get("portable:multipart");
  const aborted = await bucket.createMultipartUpload("portable:aborted");
  await aborted.abort();
  return Response.json({
    identity: upload.key === resumed.key && upload.uploadId === resumed.uploadId,
    part: { number: part.partNumber, etag: part.etag === "5d41402abc4b2a76b9719d911017c592" },
    object: {
      key: completed.key,
      size: completed.size,
      etag: completed.etag === "62109206880d38a4010a98e11243924a-1",
      storageClass: completed.storageClass,
      contentType: completed.httpMetadata?.contentType,
      customTag: completed.customMetadata?.tag,
      text: body === null ? null : await body.text(),
    },
    aborted: true,
  });
}

async function errors(bucket: R2Bucket): Promise<Response> {
  await bucket.put("portable:error-seed", "hello", {
    httpMetadata: { contentType: "text/plain" },
    customMetadata: { tag: "seed" },
  });
  for (const key of ["portable:limit-0", "portable:limit-1", "portable:limit-2", "portable:limit-3"]) {
    await bucket.put(key, key);
  }
  const body = await bucket.get("portable:error-seed");
  await body!.text();
  const listedWithoutInclude = await bucket.list({ prefix: "portable:error-seed" });
  const listedHead = listedWithoutInclude.objects[0]!;
  const observed = {
    getUnknownOption: await capture(() => invoke(bucket.get, bucket, ["portable:error-seed", { unknown: true }])),
    putUnknownOption: await capture(() => invoke(bucket.put, bucket, ["portable:unknown", "value", { unknown: true }])),
    listUnknownOption: await capture(() => invoke(bucket.list, bucket, [{ unknown: true }])),
    getNumberKey: await capture(() => invoke(bucket.get, bucket, [1])),
    getSymbolKey: await capture(() => invoke(bucket.get, bucket, [Symbol("key")])),
    negativeOffset: await capture(() => bucket.get("portable:error-seed", { range: { offset: -1 } })),
    fractionalOffset: await capture(() => bucket.get("portable:error-seed", { range: { offset: 0.5 } })),
    negativeLength: await capture(() => bucket.get("portable:error-seed", { range: { length: -1 } })),
    fractionalLength: await capture(() => bucket.get("portable:error-seed", { range: { length: 0.5 } })),
    negativeSuffix: await capture(() => bucket.get("portable:error-seed", { range: { suffix: -1 } })),
    fractionalSuffix: await capture(() => bucket.get("portable:error-seed", { range: { suffix: 0.5 } })),
    suffixOffset: await capture(() => invoke(bucket.get, bucket, ["portable:error-seed", { range: { suffix: 1, offset: 0 } }])),
    suffixLength: await capture(() => invoke(bucket.get, bucket, ["portable:error-seed", { range: { suffix: 1, length: 1 } }])),
    emptyRange: await capture(() => bucket.get("portable:error-seed", { range: {} as R2Range })),
    malformedRangeHeader: await capture(() => bucket.get("portable:error-seed", { range: new Headers({ range: "bananas" }) })),
    multiRangeHeader: await capture(() => bucket.get("portable:error-seed", { range: new Headers({ range: "bytes=0-1,3-4" }) })),
    quotedMatch: await capture(() => bucket.get("portable:error-seed", { onlyIf: { etagMatches: "\"quoted\"" } })),
    malformedMatchHeader: await capture(() => bucket.get("portable:error-seed", { onlyIf: new Headers({ "if-match": "bad" }) })),
    md5Bytes: await capture(() => bucket.put("portable:error-seed", "x", { md5: new Uint8Array(15) })),
    md5Length: await capture(() => bucket.put("portable:error-seed", "x", { md5: "00" })),
    md5Hex: await capture(() => bucket.put("portable:error-seed", "x", { md5: "z".repeat(32) })),
    multipleHashes: await capture(() => bucket.put("portable:error-seed", "x", {
      md5: "00".repeat(16), sha1: "00".repeat(20),
    })),
    ssecFormat: await capture(() => bucket.get("portable:error-seed", { ssecKey: "Z".repeat(64) })),
    ssecLength: await capture(() => bucket.get("portable:error-seed", { ssecKey: "00" })),
    invalidStorageClass: await capture(() => bucket.put("portable:storage", "value", { storageClass: "Banana" })),
    invalidInclude: await capture(() => invoke(bucket.list, bucket, [{ include: ["etag"] }])),
    writeUnknownMetadata: await capture(() => listedHead.writeHttpMetadata(new Headers())),
    bodySecondUse: await capture(() => body!.bytes()),
    unpairedKey: await capture(() => invoke(bucket.put, bucket, ["portable:\ud800", "value"])),
  };
  const zero = await bucket.list({ prefix: "portable:limit-", limit: 0 });
  const negative = await bucket.list({ prefix: "portable:limit-", limit: -1 });
  const fractional = await bucket.list({ prefix: "portable:limit-", limit: 1.5 });
  const high = await bucket.list({ prefix: "portable:limit-", limit: 1001 });
  const upload = await bucket.createMultipartUpload("portable:error-multipart");
  const multipartErrors = {
    uploadPartZero: await capture(() => upload.uploadPart(0, "x")),
    uploadPartHigh: await capture(() => upload.uploadPart(10001, "x")),
    uploadPartFractional: await capture(() => invoke(upload.uploadPart, upload, [1.5, "x"])),
    completePartZero: await capture(() => invoke(upload.complete, upload, [[{ partNumber: 0, etag: "x" }]])),
    completeNonArray: await capture(() => invoke(upload.complete, upload, [{}])),
    resumeEmpty: await capture(() => invoke(bucket.resumeMultipartUpload, bucket, ["portable:error-multipart", ""])),
  };
  await upload.abort();
  return Response.json({
    observed,
    limits: {
      zero: { count: zero.objects.length, truncated: zero.truncated, hasCursor: "cursor" in zero },
      negative: { keys: negative.objects.map(object => object.key), truncated: negative.truncated, hasCursor: "cursor" in negative },
      fractional: { keys: fractional.objects.map(object => object.key), truncated: fractional.truncated, hasCursor: "cursor" in fractional },
      high: { keys: high.objects.map(object => object.key), truncated: high.truncated, hasCursor: "cursor" in high },
    },
    multipart: multipartErrors,
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const path = new URL(request.url).pathname;
    if (path === "/reset") return reset(env.BUCKET);
    if (path === "/surface") return surface(env.BUCKET);
    if (path === "/multipart") return multipart(env.BUCKET);
    if (path === "/errors") return errors(env.BUCKET);
    if (path === "/cleanup") {
      await clear(env.BUCKET);
      return Response.json({ cleaned: true });
    }
    return new Response("not found", { status: 404 });
  },
} satisfies ExportedHandler<Env>;
