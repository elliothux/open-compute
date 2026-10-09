import type { Env } from "../env";
import { jsonError, ok } from "../lib/json";
import { withR2Retry } from "../lib/r2-retry";

const MULTIPART_THRESHOLD_BYTES = 5 * 1024 * 1024;

async function putMultipart(
  bucket: R2Bucket,
  key: string,
  body: ArrayBuffer,
): Promise<{ parts: number }> {
  const upload = await bucket.createMultipartUpload(key, {
    httpMetadata: { contentType: "application/octet-stream" },
  });
  const partSize = 5 * 1024 * 1024;
  const uploaded: R2UploadedPart[] = [];
  let partNumber = 1;
  for (let offset = 0; offset < body.byteLength; offset += partSize) {
    const chunk = body.slice(offset, offset + partSize);
    const part = await upload.uploadPart(partNumber, chunk);
    uploaded.push(part);
    partNumber += 1;
  }
  await upload.complete(uploaded);
  return { parts: uploaded.length };
}

export async function handleR2Object(
  request: Request,
  env: Env,
  key: string,
): Promise<Response> {
  if (request.method === "PUT") {
    const forceMultipart =
      new URL(request.url).searchParams.get("multipart") === "1";
    const bodyBuffer = await request.arrayBuffer();
    const useMultipart =
      forceMultipart || bodyBuffer.byteLength >= MULTIPART_THRESHOLD_BYTES;

    if (useMultipart) {
      const multipart = await putMultipart(env.BUCKET, key, bodyBuffer);
      return ok({
        stack: "r2",
        key,
        bytes: bodyBuffer.byteLength,
        multipart: true,
        ...multipart,
      });
    }

    await env.BUCKET.put(key, bodyBuffer, {
      httpMetadata: {
        contentType: request.headers.get("content-type") ?? "text/plain",
      },
    });
    return ok({
      stack: "r2",
      key,
      bytes: bodyBuffer.byteLength,
      multipart: false,
    });
  }

  if (request.method === "GET") {
    const object = await withR2Retry(() => env.BUCKET.get(key));
    if (!object) {
      return jsonError("not_found", 404, { stack: "r2", key });
    }
    const text = await withR2Retry(() => object.text());
    return ok({
      stack: "r2",
      key,
      bytes: object.size,
      etag: object.etag,
      text: text.length > 4096 ? `${text.slice(0, 4096)}…` : text,
      truncated: text.length > 4096,
    });
  }

  return jsonError("method_not_allowed", 405);
}
