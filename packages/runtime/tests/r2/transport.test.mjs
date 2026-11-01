import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { pathToFileURL } from "node:url";
import { compileRuntime, moduleUrl } from "../compiled-runtime.mjs";

const workerBase = moduleUrl(
  "export class WorkerEntrypoint { constructor(ctx, env) { this.ctx = ctx; this.env = env; } }",
);
const privateHelpers = moduleUrl(
  await compileRuntime("bindings/private-transport.ts", {
    "../loader/shared.js": moduleUrl(
      "export function bindingError(code) { return Object.assign(new Error(code), {stableCode: code}); }",
    ),
  }),
);
const nativeAdapter = moduleUrl(
  await compileRuntime("r2/native-adapter.ts", {
    "../bindings/private-transport.js": privateHelpers,
    "./validation.js": moduleUrl(await compileRuntime("r2/validation.ts")),
  }),
);
const { nativeR2Fetch } = await import(nativeAdapter);
const compiled = await mkdtemp(join(tmpdir(), "oc-r2-runtime-"));
await writeFile(
  join(compiled, "transport.mjs"),
  await compileRuntime("r2/transport.ts", {
    "cloudflare:workers": workerBase,
    "./native-adapter.js": nativeAdapter,
  }),
);
await writeFile(
  join(compiled, "validation.js"),
  await compileRuntime("r2/validation.ts"),
);
const { makeR2TransportBase } = await import(
  pathToFileURL(join(compiled, "transport.mjs")).href
);
const props = {
  bindingId: "binding",
  versionId: "version",
  descriptorSha256: "a".repeat(64),
  resourceSpecGeneration: 1,
  permissions: { read: true, write: true },
};
const meta = {
  key: "asset",
  version: "00000000-0000-7000-8000-000000000001",
  size: 3,
  etag: "etag",
  httpEtag: '"etag"',
  uploaded: 1,
  httpMetadata: { contentType: "text/plain", cacheExpiry: null },
  customMetadata: {},
  checksums: { md5: "5d41402abc4b2a76b9719d911017c592" },
  storageClass: "Standard",
};
const Transport = makeR2TransportBase((code) =>
  Object.assign(new Error(code), { stableCode: code }),
);
const transport = (fetch) =>
  new Transport({ props }, { BINDING_BACKEND: { fetch } });

test("native R2 list caps oversized limits and preserves zero", async () => {
  const limits = [];
  const source = transport(async (_url, init) => {
    limits.push(JSON.parse(init.body).limit);
    return Response.json({
      objects: [],
      truncated: false,
      delimitedPrefixes: [],
    });
  });
  for (const limit of [0, 1, 1000, 1001, 4294967295]) {
    const response = await source.fetch(
      new Request("https://fake-host/", {
        headers: {
          "cf-r2-request": JSON.stringify({
            version: 1,
            method: "list",
            limit,
          }),
        },
      }),
    );
    assert.equal(response.status, 200);
    await response.arrayBuffer();
  }
  assert.deepEqual(limits, [0, 1, 1000, 1000, 1000]);
});

test("native R2 accepts empty ranges and native surrogate bytes and reports storage errors", async () => {
  const calls = [];
  const bucket = {
    async get(key, options) {
      calls.push({ key, options });
      return { meta, body: new Blob(["abc"]).stream() };
    },
    async put(key, body) {
      calls.push({ key, body: await new Response(body).text() });
      return { ...meta, key };
    },
  };
  const get = await nativeR2Fetch(
    new Request("https://fake-host/", {
      headers: {
        "cf-r2-request": JSON.stringify({
          version: 1,
          method: "get",
          object: "asset",
          range: {},
        }),
      },
    }),
    bucket,
  );
  assert.equal(get.status, 200);
  await get.arrayBuffer();
  assert.deepEqual(calls, [{ key: "asset", options: {} }]);
  for (const storageClass of [undefined, "Banana"]) {
    const prefix = Buffer.concat([
      Buffer.from('{"version":1,"method":"put","object":"portable:'),
      Buffer.from([0xed, 0xa0, 0x80]),
      Buffer.from(
        `"${storageClass === undefined ? "" : ',"storageClass":"Banana"'}}`,
      ),
    ]);
    const response = await nativeR2Fetch(
      new Request("https://fake-host/", {
        method: "PUT",
        headers: { "cf-r2-metadata-size": String(prefix.length) },
        body: Buffer.concat([prefix, Buffer.from("value")]),
      }),
      bucket,
    );
    if (storageClass === undefined) {
      assert.equal(response.status, 200);
      assert.equal((await response.json()).name, "portable:\ufffd\ufffd\ufffd");
      assert.deepEqual(calls.at(-1), {
        key: "portable:\ufffd\ufffd\ufffd",
        body: "value",
      });
    } else {
      assert.equal(response.status, 500);
      assert.equal(
        JSON.parse(response.headers.get("cf-r2-error")).message,
        "We encountered an internal error. Please try again.",
      );
      assert.equal(calls.length, 2);
    }
  }
});

test("native R2 list shrinks oversized metadata pages without losing authority cursors", async () => {
  const calls = [];
  const all = Array.from({ length: 1000 }, (_, index) => ({
    ...meta,
    key: `key-${String(index).padStart(4, "0")}`,
    customMetadata: { value: "x".repeat(2000) },
  }));
  const source = transport(async (_url, init) => {
    const options = JSON.parse(init.body);
    calls.push(options);
    const after = Number(options.cursor ?? 0);
    const objects = all.slice(after, after + options.limit);
    const end = after + objects.length;
    return Response.json({
      objects,
      truncated: end < all.length,
      ...(end < all.length ? { cursor: String(end) } : {}),
      delimitedPrefixes: [],
    });
  });
  let cursor;
  const keys = [];
  do {
    const response = await source.fetch(
      new Request("https://fake-host/", {
        headers: {
          "cf-r2-request": JSON.stringify({
            version: 1,
            method: "list",
            limit: 1000,
            include: [1],
            ...(cursor === undefined ? {} : { cursor }),
          }),
        },
      }),
    );
    assert.equal(response.status, 200);
    assert.ok(
      Number(response.headers.get("cf-r2-metadata-size")) <= 1024 * 1024,
    );
    const page = await response.json();
    keys.push(...page.objects.map((object) => object.name));
    cursor = page.cursor;
    assert.equal(page.truncated, cursor !== undefined);
  } while (cursor !== undefined);
  assert.deepEqual(
    keys,
    all.map((object) => object.key),
  );
  assert.deepEqual(
    calls.map(({ limit, cursor }) => [limit, cursor]),
    [
      [1000, undefined],
      [500, undefined],
      [250, undefined],
      [1000, "250"],
      [500, "250"],
      [250, "250"],
      [1000, "500"],
      [500, "500"],
      [250, "500"],
      [1000, "750"],
    ],
  );
  // A malformed single metadata object fails closed; it cannot trigger unbounded retries.
  const oversized = transport(async () =>
    Response.json({
      objects: [
        { ...meta, customMetadata: { value: "x".repeat(1024 * 1024) } },
      ],
      truncated: false,
      delimitedPrefixes: [],
    }),
  );
  const invalid = await oversized.fetch(
    new Request("https://fake-host/", {
      headers: { "cf-r2-request": '{"version":1,"method":"list","limit":1}' },
    }),
  );
  assert.equal(invalid.status, 500);
  assert.equal(
    JSON.parse(invalid.headers.get("cf-r2-error")).message,
    "R2_INTERNAL_PROTOCOL_ERROR",
  );
});

test("native R2 cancellation reaches GET authority and malformed PUT input", async () => {
  let outputCancelled = false;
  const source = transport(async () => {
    const json = new TextEncoder().encode(
      JSON.stringify({ meta, hasBody: true }),
    );
    const prefix = new Uint8Array(4 + json.length);
    new DataView(prefix.buffer).setUint32(0, json.length);
    prefix.set(json, 4);
    return new Response(
      new ReadableStream({
        start(controller) {
          controller.enqueue(prefix);
        },
        cancel() {
          outputCancelled = true;
        },
      }),
      {
        headers: { "content-type": "application/vnd.open-compute.r2.v1+frame" },
      },
    );
  });
  const response = await source.fetch(
    new Request("https://fake-host/", {
      headers: {
        "cf-r2-request": '{"version":1,"method":"get","object":"asset"}',
      },
    }),
  );
  const reader = response.body.getReader();
  assert.ok((await reader.read()).value.length > 0);
  await reader.cancel("finished");
  assert.equal(outputCancelled, true);
  let inputCancelled = false;
  const invalid = await source.fetch(
    new Request("https://fake-host/", {
      method: "PUT",
      duplex: "half",
      headers: { "cf-r2-metadata-size": "1" },
      body: new ReadableStream({
        start(controller) {
          controller.enqueue(new Uint8Array([255]));
        },
        cancel() {
          inputCancelled = true;
        },
      }),
    }),
  );
  assert.equal(invalid.status, 400);
  assert.equal(inputCancelled, true);
});

test("native R2 wire reuses the authority for streams, metadata, conditions and multipart", async () => {
  const calls = [];
  const source = transport(async (url, init) => {
    const operation = String(url).split("/").at(-1);
    let input;
    let body;
    if (operation === "put" || operation === "uploadPart") {
      const bytes = new Uint8Array(await new Response(init.body).arrayBuffer());
      const length = new DataView(bytes.buffer).getUint32(0);
      input = JSON.parse(
        new TextDecoder().decode(bytes.subarray(4, 4 + length)),
      );
      body = new TextDecoder().decode(bytes.subarray(4 + length));
    } else input = JSON.parse(init.body);
    calls.push({ operation, input, body });
    if (operation === "get")
      return frame(meta, input.options.onlyIf ? undefined : "abc");
    if (operation === "list")
      return Response.json({
        objects: [meta],
        truncated: true,
        cursor: "cursor",
        delimitedPrefixes: ["prefix/"],
      });
    if (operation === "head" && input.key === "missing")
      return new Response(null, { status: 204 });
    if (operation === "put" && input.options.onlyIf)
      return new Response(null, { status: 204 });
    if (operation === "createMultipartUpload")
      return Response.json({ key: input.key, uploadId: "upload" });
    if (operation === "uploadPart")
      return Response.json({ partNumber: input.partNumber, etag: "part" });
    if (operation === "delete" || operation === "abortMultipartUpload")
      return new Response(null, { status: 204 });
    return Response.json(meta);
  });
  function request(method, input = {}, body = "") {
    const value = { version: 1, method, object: "路径/%", ...input };
    const metadata = new TextEncoder().encode(JSON.stringify(value));
    const get = ["head", "get", "list"].includes(method);
    return new Request(
      "https://fake-host/",
      get
        ? {
            headers: {
              "cf-r2-request": JSON.stringify(value).replace(
                /[\u007f-\uffff]/g,
                (char) =>
                  `\\u${char.charCodeAt(0).toString(16).padStart(4, "0")}`,
              ),
            },
          }
        : {
            method: "PUT",
            duplex: "half",
            headers: { "cf-r2-metadata-size": String(metadata.length) },
            body: new ReadableStream({
              start(controller) {
                controller.enqueue(metadata.subarray(0, 2));
                controller.enqueue(metadata.subarray(2));
                if (body) controller.enqueue(new TextEncoder().encode(body));
                controller.close();
              },
            }),
          },
    );
  }
  async function decoded(response) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    const size = Number(
      response.headers.get("cf-r2-metadata-size") ?? bytes.length,
    );
    return {
      metadata: JSON.parse(new TextDecoder().decode(bytes.subarray(0, size))),
      body: new TextDecoder().decode(bytes.subarray(size)),
    };
  }
  const get = await source.fetch(
    request("get", {
      range: { offset: "1", length: "2" },
      ssec: { key: "a".repeat(64) },
    }),
  );
  const result = await decoded(get);
  assert.equal(result.body, "abc");
  assert.equal(result.metadata.name, "asset");
  assert.equal(result.metadata.size, "3");
  assert.deepEqual(result.metadata.checksums, { 0: meta.checksums.md5 });
  assert.equal(result.metadata.httpFields.contentType, "text/plain");
  assert.deepEqual(calls.at(-1).input.options, {
    range: { offset: 1, length: 2 },
    ssecKey: "a".repeat(64),
  });
  const conditional = await source.fetch(
    request("get", {
      onlyIf: {
        etagMatches: [{ type: "weak", value: "etag" }],
        uploadedBefore: "1",
        secondsGranularity: true,
      },
    }),
  );
  assert.equal(conditional.status, 412);
  assert.equal(
    JSON.parse(conditional.headers.get("cf-r2-error")).v4code,
    10031,
  );
  assert.equal((await decoded(conditional)).body, "");
  assert.deepEqual(calls.at(-1).input.options.onlyIf, {
    etagMatches: [{ kind: "weak", value: "etag" }],
    etagDoesNotMatch: [],
    uploadedBefore: 1,
    secondsGranularity: true,
  });
  await source.fetch(request("get", { rangeHeader: "bytes=1-2" }));
  assert.deepEqual(calls.at(-1).input.options.range, { offset: 1, length: 2 });
  const put = await source.fetch(
    request(
      "put",
      {
        customFields: [{ k: "__proto__", v: "value" }],
        httpFields: { cacheExpiry: "123", contentType: "text/plain" },
        md5: "AAAAAAAAAAAAAAAAAAAAAA==",
      },
      "abc",
    ),
  );
  assert.equal((await put.json()).etag, "etag");
  assert.equal(calls.at(-1).body, "abc");
  assert.deepEqual(calls.at(-1).input.options, {
    httpMetadata: { cacheExpiry: 123, contentType: "text/plain" },
    customMetadata: JSON.parse('{"__proto__":"value"}'),
    checksum: { algorithm: "md5", hex: "0".repeat(32) },
  });
  assert.equal(
    (
      await source.fetch(
        request(
          "put",
          { onlyIf: { etagDoesNotMatch: [{ type: "wildcard" }] } },
          "abc",
        ),
      )
    ).status,
    412,
  );
  const missing = await source.fetch(request("head", { object: "missing" }));
  assert.equal(missing.status, 404);
  assert.equal(JSON.parse(missing.headers.get("cf-r2-error")).v4code, 10007);
  const listed = await decoded(
    await source.fetch(
      request("list", {
        prefix: "p/",
        limit: 10,
        include: [0, 1],
        cursor: "previous",
        delimiter: "/",
        startAfter: "p/a",
      }),
    ),
  );
  assert.equal(listed.metadata.cursor, "cursor");
  assert.equal(listed.metadata.objects[0].size, "3");
  assert.deepEqual(calls.at(-1).input.include, [
    "httpMetadata",
    "customMetadata",
  ]);
  assert.deepEqual(
    await (await source.fetch(request("createMultipartUpload"))).json(),
    { uploadId: "upload" },
  );
  assert.deepEqual(
    await (
      await source.fetch(
        request(
          "uploadPart",
          { uploadId: "upload", partNumber: 1 },
          "part-body",
        ),
      )
    ).json(),
    { etag: "part" },
  );
  assert.equal(calls.at(-1).body, "part-body");
  assert.equal(
    (
      await (
        await source.fetch(
          request("completeMultipartUpload", {
            uploadId: "upload",
            parts: [{ part: 1, etag: "part" }],
          }),
        )
      ).json()
    ).name,
    "asset",
  );
  assert.deepEqual(calls.at(-1).input.parts, [{ partNumber: 1, etag: "part" }]);
  await source.fetch(request("abortMultipartUpload", { uploadId: "upload" }));
  await source.fetch(request("delete", { objects: ["a", "b"] }));
  assert.deepEqual(calls.at(-1).input.keys, ["a", "b"]);
  const before = calls.length;
  for (const invalid of [
    request("head", { version: 2 }),
    request("get", { range: { offset: "-1" } }),
    request("get", { range: { unknown: 1 } }),
    request("list", { include: [2] }),
    request("put", { httpFields: { unknown: "x" } }),
    request("put", {
      customFields: [
        { k: "x", v: "a" },
        { k: "x", v: "b" },
      ],
    }),
    request("get", { ssec: { key: "secret" } }),
    request("get", { onlyIf: { etagMatches: [{ type: "bad" }] } }),
    request("put", { md5: "AA==", sha256: "a".repeat(64) }),
    request("delete", {}, "unexpected body"),
    new Request("https://fake-host/", { headers: { "cf-r2-request": "{" } }),
    new Request("https://fake-host/", {
      method: "PUT",
      body: "{}",
      headers: { "cf-r2-metadata-size": "16385" },
    }),
    new Request("https://fake-host/", {
      method: "PUT",
      body: "{}",
      headers: { "cf-r2-metadata-size": "3" },
    }),
  ])
    assert.equal((await source.fetch(invalid)).status, 400);
  assert.equal(calls.length, before);
  const denied = new Transport(
    { props: { ...props, permissions: { read: false, write: false } } },
    {},
  );
  assert.equal((await denied.fetch(request("head"))).status, 403);
  assert.equal((await denied.fetch(request("delete"))).status, 403);
  const broken = transport(async () => {
    throw new Error("SECRET_PATH_TOKEN");
  });
  const error = await broken.fetch(request("head"));
  assert.equal(error.status, 500);
  assert.equal(
    JSON.parse(error.headers.get("cf-r2-error")).message,
    "R2_INTERNAL_PROTOCOL_ERROR",
  );
});

function frame(metadata, body) {
  const header = new TextEncoder().encode(
    JSON.stringify({ meta: metadata, hasBody: body !== undefined }),
  );
  const prefix = new Uint8Array(4);
  new DataView(prefix.buffer).setUint32(0, header.length);
  return new Response(
    new ReadableStream({
      start(controller) {
        for (const part of [
          prefix,
          header,
          ...(body === undefined ? [] : [new TextEncoder().encode(body)]),
        ])
          controller.enqueue(part);
        controller.close();
      },
    }),
  );
}

test("compiled R2 transport rejects malformed metadata and incomplete frames", async () => {
  for (const invalid of [
    null,
    { ...meta, size: "3" },
    { ...meta, customMetadata: { x: 1 } },
    { ...meta, range: { offset: -1 } },
    { ...meta, httpMetadata: { cacheExpiry: "tomorrow" } },
  ]) {
    await assert.rejects(
      transport(async () => frame(invalid, "abc")).get("asset", {}),
      /BINDING_PROTOCOL_ERROR/,
    );
    await assert.rejects(
      transport(async () => Response.json(invalid)).head("asset"),
      /BINDING_PROTOCOL_ERROR/,
    );
  }
  await assert.rejects(
    transport(async () => new Response(new Uint8Array([0, 0]))).get(
      "asset",
      {},
    ),
    /BINDING_PROTOCOL_ERROR/,
  );
});
