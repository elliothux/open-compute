// Generated from runtime/src/kv/transport.ts by Rolldown. Do not edit.
import { WorkerEntrypoint } from "cloudflare:workers";
import { bindingError, currentStartupGeneration } from "../loader/host.js";
const BINDING_TOKEN_HEADER = "x-open-compute-binding-token";
const BINDING_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+json";
const BINDING_FRAME_CONTENT_TYPE = "application/vnd.open-compute.kv.v1+frame";
const MAX_BINDING_KEY_BYTES = 512;
const MAX_KV_VALUE_BYTES = 25 * 1024 * 1024;
const MAX_KV_KEYS = 100;
function record(value) {
	return value !== null && typeof value === "object" && !Array.isArray(value);
}
function assertKey(key) {
	if (typeof key !== "string" || !key || key === "." || key === "..") {
		throw new TypeError("KV_KEY_INVALID");
	}
	for (let i = 0; i < key.length; i++) {
		const code = key.charCodeAt(i);
		if (code >= 55296 && code <= 56319) {
			const next = key.charCodeAt(++i);
			if (!(next >= 56320 && next <= 57343)) throw new TypeError("KV_KEY_INVALID");
		} else if (code >= 56320 && code <= 57343) {
			throw new TypeError("KV_KEY_INVALID");
		}
	}
	if (new TextEncoder().encode(key).byteLength > MAX_BINDING_KEY_BYTES) {
		throw new TypeError("KV_KEY_TOO_LARGE");
	}
}
function assertPrefix(prefix) {
	if (typeof prefix !== "string") throw new TypeError("KV_INVALID_OPTIONS");
	for (let i = 0; i < prefix.length; i++) {
		const code = prefix.charCodeAt(i);
		if (code >= 55296 && code <= 56319) {
			const next = prefix.charCodeAt(++i);
			if (!(next >= 56320 && next <= 57343)) throw new TypeError("KV_KEY_INVALID");
		} else if (code >= 56320 && code <= 57343) {
			throw new TypeError("KV_KEY_INVALID");
		}
	}
	if (new TextEncoder().encode(prefix).byteLength > MAX_BINDING_KEY_BYTES) {
		throw new TypeError("KV_KEY_TOO_LARGE");
	}
}
function assertSafeSeconds(value, minimum) {
	if (typeof value !== "number" || !Number.isSafeInteger(value) || value < minimum) throw new TypeError("KV_INVALID_OPTIONS");
}
function getOptions(input, many) {
	let type = "text";
	let cacheTtl;
	if (input !== undefined) {
		if (typeof input === "string") type = input;
		else if (record(input)) {
			const keys = Object.keys(input);
			if (keys.some((key) => key !== "type" && key !== "cacheTtl")) {
				throw new TypeError("KV_INVALID_OPTIONS");
			}
			if (input.type !== undefined) type = input.type;
			if (input.cacheTtl !== undefined) {
				assertSafeSeconds(input.cacheTtl, 30);
				cacheTtl = input.cacheTtl;
			}
		} else throw new TypeError("KV_INVALID_OPTIONS");
	}
	if (type !== "text" && type !== "json" && type !== "arrayBuffer" && type !== "stream" || many && type !== "text" && type !== "json") throw new TypeError("KV_INVALID_OPTIONS");
	return {
		type,
		cacheTtl
	};
}
function assertMetadata(value, seen = new WeakSet()) {
	if (value === null || typeof value === "string" || typeof value === "boolean") return;
	if (typeof value === "number") {
		if (!Number.isFinite(value)) throw new TypeError("KV_METADATA_INVALID");
		return;
	}
	if (typeof value !== "object") throw new TypeError("KV_METADATA_INVALID");
	if (seen.has(value)) throw new TypeError("KV_METADATA_INVALID");
	seen.add(value);
	if (Array.isArray(value)) {
		for (const entry of value) assertMetadata(entry, seen);
	} else {
		for (const item of Object.values(value)) assertMetadata(item, seen);
	}
	seen.delete(value);
}
function putOptions(input) {
	if (input === undefined) return { metadataPresent: false };
	if (!record(input)) {
		throw new TypeError("KV_INVALID_OPTIONS");
	}
	const keys = Object.keys(input);
	if (keys.some((key) => ![
		"expiration",
		"expirationTtl",
		"metadata"
	].includes(key))) {
		throw new TypeError("KV_INVALID_OPTIONS");
	}
	if (input.expiration !== undefined && input.expirationTtl !== undefined) {
		throw new TypeError("KV_INVALID_OPTIONS");
	}
	if (input.expiration !== undefined) assertSafeSeconds(input.expiration, 1);
	if (input.expirationTtl !== undefined) assertSafeSeconds(input.expirationTtl, 60);
	const metadataPresent = Object.prototype.hasOwnProperty.call(input, "metadata") && input.metadata !== undefined;
	if (metadataPresent) assertMetadata(input.metadata);
	return {
		expiration: input.expiration,
		expirationTtl: input.expirationTtl,
		metadata: metadataPresent ? input.metadata : undefined,
		metadataPresent
	};
}
function valueStream(value) {
	if (typeof value === "string") {
		const bytes = new TextEncoder().encode(value);
		return {
			stream: new Blob([bytes]).stream(),
			knownLength: bytes.byteLength
		};
	}
	if (value instanceof ArrayBuffer) {
		const bytes = new Uint8Array(value);
		return {
			stream: new Blob([bytes]).stream(),
			knownLength: bytes.byteLength
		};
	}
	if (ArrayBuffer.isView(value)) {
		const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
		return {
			stream: new Blob([bytes]).stream(),
			knownLength: bytes.byteLength
		};
	}
	if (value instanceof ReadableStream) return {
		stream: value,
		knownLength: undefined
	};
	throw new TypeError("KV value must be a string, buffer, view, or ReadableStream");
}
function framedPutBody(header, value) {
	const headerBytes = new TextEncoder().encode(JSON.stringify(header));
	if (headerBytes.byteLength > 4096) throw new TypeError("KV_METADATA_TOO_LARGE");
	const prefix = new Uint8Array(4 + headerBytes.byteLength);
	new DataView(prefix.buffer).setUint32(0, headerBytes.byteLength);
	prefix.set(headerBytes, 4);
	const source = valueStream(value);
	if (source.knownLength !== undefined && source.knownLength > MAX_KV_VALUE_BYTES) {
		throw new TypeError("KV_VALUE_TOO_LARGE");
	}
	const reader = source.stream.getReader();
	let first = true;
	let total = 0;
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
				controller.error(new TypeError("KV stream chunks must be bytes"));
				return;
			}
			total += next.value.byteLength;
			if (total > MAX_KV_VALUE_BYTES) {
				const prior = total - next.value.byteLength;
				const firstOverflowByte = next.value.subarray(0, MAX_KV_VALUE_BYTES - prior + 1);
				controller.enqueue(firstOverflowByte);
				await reader.cancel();
				controller.close();
				return;
			}
			controller.enqueue(next.value);
		},
		cancel(reason) {
			return reader.cancel(reason);
		}
	});
}
function decodeEntry(view, state) {
	if (state.offset + 17 > view.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
	const found = view.getUint8(state.offset++);
	const expiration = view.getBigInt64(state.offset);
	state.offset += 8;
	const metadataLength = view.getUint32(state.offset);
	state.offset += 4;
	let metadata = null;
	if (metadataLength !== 4294967295) {
		if (state.offset + metadataLength > view.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		metadata = JSON.parse(new TextDecoder().decode(new Uint8Array(view.buffer, view.byteOffset + state.offset, metadataLength)));
		state.offset += metadataLength;
	}
	const valueLength = view.getUint32(state.offset);
	state.offset += 4;
	if (!found) {
		if (valueLength !== 4294967295) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		return {
			value: null,
			metadata: null,
			expiration: null
		};
	}
	if (valueLength === 4294967295 || state.offset + valueLength > view.byteLength) {
		throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
	}
	const value = new Uint8Array(valueLength);
	value.set(new Uint8Array(view.buffer, view.byteOffset + state.offset, valueLength));
	state.offset += valueLength;
	return {
		value,
		metadata,
		expiration: expiration < 0n ? null : Number(expiration)
	};
}
function decodeValue(bytes, type) {
	if (bytes === null) return null;
	if (type === "arrayBuffer") return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength);
	const text = new TextDecoder().decode(bytes);
	if (type === "json") return JSON.parse(text);
	return text;
}
async function decodeStreamValue(stream, type) {
	if (stream === null || type === "stream") return stream;
	const response = new Response(stream);
	if (type === "arrayBuffer") return response.arrayBuffer();
	const text = await response.text();
	if (type === "json") return JSON.parse(text);
	return text;
}
async function decodeSingleEntry(response) {
	if (!response.body) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
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
					throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
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
	const prefix = await exact(17);
	if (new TextDecoder().decode(prefix.subarray(0, 4)) !== "KVS1") {
		throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
	}
	const view = new DataView(prefix.buffer, prefix.byteOffset, prefix.byteLength);
	const found = view.getUint8(4);
	const expiration = view.getBigInt64(5);
	const metadataLength = view.getUint32(13);
	let metadata = null;
	if (metadataLength !== 4294967295) {
		metadata = JSON.parse(new TextDecoder().decode(await exact(metadataLength)));
	}
	const valueLength = new DataView((await exact(4)).buffer).getUint32(0);
	if (!found) {
		if (valueLength !== 4294967295 || metadataLength !== 4294967295) {
			throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		}
		const terminal = await reader.read();
		if (!terminal.done) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		return {
			value: null,
			metadata: null,
			expiration: null
		};
	}
	if (valueLength === 4294967295) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
	let remaining = valueLength;
	const value = new ReadableStream({
		async pull(controller) {
			if (remaining === 0) {
				const terminal = await reader.read();
				if (!terminal.done) {
					controller.error(bindingError("KV_INTERNAL_PROTOCOL_ERROR"));
					return;
				}
				controller.close();
				return;
			}
			if (offset < buffered.byteLength) {
				const count = Math.min(remaining, buffered.byteLength - offset);
				controller.enqueue(buffered.subarray(offset, offset + count));
				offset += count;
				remaining -= count;
				return;
			}
			const next = await reader.read();
			if (next.done || !(next.value instanceof Uint8Array) || next.value.byteLength > remaining) {
				controller.error(bindingError("KV_INTERNAL_PROTOCOL_ERROR"));
				return;
			}
			remaining -= next.value.byteLength;
			controller.enqueue(next.value);
		},
		cancel(reason) {
			remaining = 0;
			return reader.cancel(reason);
		}
	});
	return {
		value,
		metadata,
		expiration: expiration < 0n ? null : Number(expiration)
	};
}
export class KVNamespace extends WorkerEntrypoint {
	#props() {
		const props = this.ctx.props;
		if (!props || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string" || !/^[0-9a-f]{64}$/.test(props.descriptorSha256) || !Number.isSafeInteger(props.resourceSpecGeneration) || props.resourceSpecGeneration < 1) {
			throw bindingError("BINDING_PROTOCOL_ERROR");
		}
		return props;
	}
	async #request(operation, body, permission, contentType = BINDING_CONTENT_TYPE) {
		const props = this.#props();
		if (!props.permissions[permission]) {
			throw bindingError("BINDING_PERMISSION_DENIED");
		}
		const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend/internal/bindings/v1/kv/${props.bindingId}/${operation}`, {
			method: "POST",
			headers: {
				"content-type": contentType,
				[BINDING_TOKEN_HEADER]: this.env.BINDING_BACKEND_TOKEN,
				"x-open-compute-startup-generation": currentStartupGeneration(),
				"x-open-compute-deployment-id": props.deploymentId,
				"x-open-compute-descriptor-sha256": props.descriptorSha256,
				"x-open-compute-request-id": crypto.randomUUID()
			},
			body
		});
		if (!response.ok) {
			const code = response.headers.get("x-open-compute-error-code") || "BINDING_PROTOCOL_ERROR";
			try {
				await response.body?.cancel();
			} catch {}
			throw bindingError(code);
		}
		return response;
	}
	async #entries(operation, keys, options) {
		const response = await this.#request(operation, JSON.stringify({
			keys,
			cacheTtl: options.cacheTtl
		}), "read", BINDING_FRAME_CONTENT_TYPE);
		if (operation === "get" || operation === "get-with-metadata") {
			return [await decodeSingleEntry(response)];
		}
		const buffer = await response.arrayBuffer();
		const view = new DataView(buffer);
		const magic = new TextDecoder().decode(new Uint8Array(buffer, 0, 4));
		const state = { offset: 4 };
		if (magic !== "KVB1" || buffer.byteLength < 6) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		const count = view.getUint16(4);
		state.offset = 6;
		if (count !== keys.length) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		const entries = [];
		for (let i = 0; i < count; i++) entries.push(decodeEntry(view, state));
		if (state.offset !== buffer.byteLength) throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		return entries;
	}
	async get(keyOrKeys, typeOrOptions) {
		const many = Array.isArray(keyOrKeys);
		const keys = many ? keyOrKeys : [keyOrKeys];
		if (many && keys.length > MAX_KV_KEYS) throw new TypeError("KV_TOO_MANY_KEYS");
		for (const key of keys) assertKey(key);
		const options = getOptions(typeOrOptions, many);
		if (!many) {
			const [entry] = await this.#entries("get", keys, options);
			return decodeStreamValue(entry.value, options.type);
		}
		const entries = await this.#entries("get-many", keys, options);
		const result = new Map();
		for (let i = 0; i < keys.length; i++) {
			if (!result.has(keys[i])) result.set(keys[i], decodeValue(entries[i].value, options.type));
		}
		return result;
	}
	async getWithMetadata(keyOrKeys, typeOrOptions) {
		const many = Array.isArray(keyOrKeys);
		const keys = many ? keyOrKeys : [keyOrKeys];
		if (many && keys.length > MAX_KV_KEYS) throw new TypeError("KV_TOO_MANY_KEYS");
		for (const key of keys) assertKey(key);
		const options = getOptions(typeOrOptions, many);
		if (!many) {
			const [entry] = await this.#entries("get-with-metadata", keys, options);
			return {
				value: await decodeStreamValue(entry.value, options.type),
				metadata: entry.metadata
			};
		}
		const entries = await this.#entries("get-many", keys, options);
		const convert = (entry) => ({
			value: decodeValue(entry.value, options.type),
			metadata: entry.metadata
		});
		const result = new Map();
		for (let i = 0; i < keys.length; i++) {
			if (!result.has(keys[i])) result.set(keys[i], convert(entries[i]));
		}
		return result;
	}
	async put(key, value, options) {
		assertKey(key);
		const normalized = putOptions(options);
		const header = {
			key,
			expiration: normalized.expiration,
			expirationTtl: normalized.expirationTtl,
			metadata: normalized.metadata,
			metadataPresent: normalized.metadataPresent
		};
		await this.#request("put", framedPutBody(header, value), "write", BINDING_FRAME_CONTENT_TYPE);
	}
	async delete(key) {
		assertKey(key);
		await this.#request("delete", JSON.stringify({ key }), "write", BINDING_FRAME_CONTENT_TYPE);
	}
	async list(options = {}) {
		if (!record(options)) {
			throw new TypeError("KV_INVALID_OPTIONS");
		}
		if (Object.keys(options).some((key) => ![
			"prefix",
			"limit",
			"cursor"
		].includes(key))) {
			throw new TypeError("KV_INVALID_OPTIONS");
		}
		const prefix = options.prefix === undefined ? "" : options.prefix;
		assertPrefix(prefix);
		const limit = options.limit === undefined ? 1e3 : options.limit;
		if (typeof limit !== "number" || !Number.isSafeInteger(limit) || limit < 1 || limit > 1e3) {
			throw new TypeError("KV_INVALID_OPTIONS");
		}
		if (options.cursor !== undefined && typeof options.cursor !== "string") {
			throw new TypeError("KV_INVALID_OPTIONS");
		}
		const response = await this.#request("list", JSON.stringify({
			prefix,
			limit,
			cursor: options.cursor
		}), "read", BINDING_FRAME_CONTENT_TYPE);
		const result = await response.json();
		if (!record(result) || !Array.isArray(result.keys) || typeof result.list_complete !== "boolean" || result.cursor !== null && typeof result.cursor !== "string") throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
		const keys = [];
		for (const key of result.keys) {
			if (!record(key) || typeof key.name !== "string" || key.expiration !== null && (typeof key.expiration !== "number" || !Number.isSafeInteger(key.expiration))) {
				throw bindingError("KV_INTERNAL_PROTOCOL_ERROR");
			}
			keys.push({
				name: key.name,
				...key.expiration === null ? {} : { expiration: key.expiration },
				...key.metadata === null ? {} : { metadata: key.metadata }
			});
		}
		return {
			keys,
			list_complete: result.list_complete,
			...result.cursor === null ? {} : { cursor: result.cursor }
		};
	}
	async echoStream(stream) {
		if (!(stream instanceof ReadableStream)) {
			throw new TypeError("binding stream must be a byte ReadableStream");
		}
		const response = await this.#request("echo", stream, "read", "application/vnd.open-compute.kv.v1+octet-stream");
		return response.body;
	}
	async fetch() {
		throw bindingError("BINDING_PERMISSION_DENIED");
	}
}
