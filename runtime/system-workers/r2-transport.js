import { WorkerEntrypoint } from "cloudflare:workers";

const CONTENT_TYPE = "application/vnd.open-compute.r2.v1+json";
const FRAME_CONTENT_TYPE = "application/vnd.open-compute.r2.v1+frame";
const MAX_METADATA_BYTES = 16384;

function framedPutBody(header, stream, bindingError) {
  if (!(stream instanceof ReadableStream)) throw bindingError("R2_INVALID_OPTIONS");
  const encoded = new TextEncoder().encode(JSON.stringify(header));
  if (encoded.byteLength > MAX_METADATA_BYTES) throw bindingError("R2_METADATA_TOO_LARGE");
  const prefix = new Uint8Array(4 + encoded.byteLength);
  new DataView(prefix.buffer).setUint32(0, encoded.byteLength);
  prefix.set(encoded, 4);
  const reader = stream.getReader();
  let first = true;
  return new ReadableStream({
    async pull(controller) {
      if (first) {
        first = false;
        controller.enqueue(prefix);
        return;
      }
      const next = await reader.read();
      if (next.done) {
        controller.close();
        return;
      }
      if (!(next.value instanceof Uint8Array)) {
        await reader.cancel();
        controller.error(bindingError("R2_INVALID_OPTIONS"));
        return;
      }
      controller.enqueue(next.value);
    },
    cancel(reason) { return reader.cancel(reason); },
  });
}

async function decodeFrame(response, bindingError) {
  if (!response.body) throw bindingError("BINDING_PROTOCOL_ERROR");
  const reader = response.body.getReader();
  let buffered = new Uint8Array(0);
  let offset = 0;
  const exact = async (length) => {
    const output = new Uint8Array(length);
    let written = 0;
    while (written < length) {
      if (offset === buffered.byteLength) {
        const next = await reader.read();
        if (next.done || !(next.value instanceof Uint8Array)) {
          throw bindingError("BINDING_PROTOCOL_ERROR");
        }
        buffered = next.value;
        offset = 0;
      }
      const count = Math.min(length - written, buffered.byteLength - offset);
      output.set(buffered.subarray(offset, offset + count), written);
      offset += count;
      written += count;
    }
    return output;
  };
  const metadataLength = new DataView((await exact(4)).buffer).getUint32(0);
  if (metadataLength > MAX_METADATA_BYTES) throw bindingError("BINDING_PROTOCOL_ERROR");
  const frame = JSON.parse(new TextDecoder().decode(await exact(metadataLength)));
  if (!frame || typeof frame !== "object" || !frame.meta) {
    throw bindingError("BINDING_PROTOCOL_ERROR");
  }
  if (frame.hasBody !== true) {
    const terminal = await reader.read();
    if (!terminal.done) throw bindingError("BINDING_PROTOCOL_ERROR");
    return { meta: frame.meta };
  }
  const body = new ReadableStream({
    async pull(controller) {
      if (offset < buffered.byteLength) {
        controller.enqueue(buffered.subarray(offset));
        offset = buffered.byteLength;
        return;
      }
      const next = await reader.read();
      if (next.done) {
        controller.close();
      } else if (next.value instanceof Uint8Array) {
        controller.enqueue(next.value);
      } else {
        controller.error(bindingError("BINDING_PROTOCOL_ERROR"));
      }
    },
    cancel(reason) { return reader.cancel(reason); },
  });
  return { meta: frame.meta, body };
}

export function makeR2TransportBase(bindingError, currentStartupGeneration, tokenHeader) {
  return class extends WorkerEntrypoint {
    #props() {
      const props = this.ctx.props;
      if (!props
        || typeof props.bindingId !== "string"
        || typeof props.deploymentId !== "string"
        || !/^[0-9a-f]{64}$/.test(props.descriptorSha256)
        || !Number.isSafeInteger(props.resourceSpecGeneration)
        || props.resourceSpecGeneration < 1) {
        throw bindingError("BINDING_PROTOCOL_ERROR");
      }
      return props;
    }

    async #request(operation, body, permission, contentType = CONTENT_TYPE) {
      const props = this.#props();
      if (!props.permissions[permission]) throw bindingError("BINDING_PERMISSION_DENIED");
      const response = await this.env.BINDING_BACKEND.fetch(
        `http://binding-backend/internal/bindings/v1/r2/${props.bindingId}/${operation}`,
        {
          method: "POST",
          headers: {
            "content-type": contentType,
            [tokenHeader]: this.env.BINDING_BACKEND_TOKEN,
            "x-open-compute-startup-generation": currentStartupGeneration(),
            "x-open-compute-deployment-id": props.deploymentId,
            "x-open-compute-descriptor-sha256": props.descriptorSha256,
            "x-open-compute-request-id": crypto.randomUUID(),
          },
          body,
        },
      );
      if (!response.ok) {
        const code = response.headers.get("x-open-compute-error-code") || "BINDING_PROTOCOL_ERROR";
        try { await response.body?.cancel(); } catch { /* best effort */ }
        throw bindingError(code);
      }
      return response;
    }

    async head(key) {
      const response = await this.#request("head", JSON.stringify({ key }), "read");
      return response.status === 204 ? null : response.json();
    }

    async get(key, options) {
      const response = await this.#request(
        "get",
        JSON.stringify({ key, options }),
        "read",
        FRAME_CONTENT_TYPE,
      );
      return response.status === 204 ? null : decodeFrame(response, bindingError);
    }

    async put(key, body, options) {
      const response = await this.#request(
        "put",
        framedPutBody({ key, options }, body, bindingError),
        "write",
        FRAME_CONTENT_TYPE,
      );
      return response.status === 204 ? null : response.json();
    }

    async delete(keys) {
      await this.#request("delete", JSON.stringify({ keys }), "write");
    }

    async list(options) {
      const response = await this.#request("list", JSON.stringify(options), "read");
      return response.json();
    }

    async fetch() {
      throw bindingError("BINDING_PERMISSION_DENIED");
    }
  };
}
