// Generated from runtime/src/d1/transport.ts by Rolldown. Do not edit.
import { WorkerEntrypoint } from "cloudflare:workers";
const FRAME_CONTENT_TYPE = "application/vnd.open-compute.d1.v1+frame";
const JSON_CONTENT_TYPE = "application/vnd.open-compute.d1.v1+json";
const MAX_FRAME_BYTES = 16 * 1024 * 1024;
const encoder = new TextEncoder();
const decoder = new TextDecoder("utf-8", {
	fatal: true,
	ignoreBOM: false
});
class FrameWriter {
	parts;
	length;
	constructor() {
		this.parts = [];
		this.length = 0;
	}
	add(bytes) {
		if (!(bytes instanceof Uint8Array)) bytes = new Uint8Array(bytes);
		this.length += bytes.byteLength;
		if (this.length > MAX_FRAME_BYTES) throw new TypeError("D1_LIMIT_ERROR");
		this.parts.push(bytes);
	}
	u8(value) {
		this.add(Uint8Array.of(value));
	}
	u16(value) {
		const out = new Uint8Array(2);
		new DataView(out.buffer).setUint16(0, value);
		this.add(out);
	}
	u32(value) {
		const out = new Uint8Array(4);
		new DataView(out.buffer).setUint32(0, value);
		this.add(out);
	}
	i64(value) {
		const out = new Uint8Array(8);
		new DataView(out.buffer).setBigInt64(0, BigInt(value));
		this.add(out);
	}
	f64(value) {
		const out = new Uint8Array(8);
		new DataView(out.buffer).setFloat64(0, value);
		this.add(out);
	}
	bytes(value) {
		this.u32(value.byteLength);
		this.add(value);
	}
	text(value) {
		this.bytes(encoder.encode(value));
	}
	finish() {
		const output = new Uint8Array(this.length);
		let offset = 0;
		for (const part of this.parts) {
			output.set(part, offset);
			offset += part.byteLength;
		}
		return output;
	}
}
class FrameReader {
	bytes;
	view;
	offset;
	bindingError;
	constructor(bytes, bindingError) {
		if (!(bytes instanceof ArrayBuffer) || bytes.byteLength > MAX_FRAME_BYTES) {
			throw bindingError("D1_INTERNAL_PROTOCOL_ERROR");
		}
		this.bytes = new Uint8Array(bytes);
		this.view = new DataView(bytes);
		this.offset = 0;
		this.bindingError = bindingError;
	}
	take(length) {
		if (!Number.isSafeInteger(length) || length < 0 || this.offset + length > this.bytes.byteLength) {
			throw this.bindingError("D1_INTERNAL_PROTOCOL_ERROR");
		}
		const value = this.bytes.subarray(this.offset, this.offset + length);
		this.offset += length;
		return value;
	}
	u8() {
		return this.take(1)[0];
	}
	u16() {
		const at = this.offset;
		this.take(2);
		return this.view.getUint16(at);
	}
	u32() {
		const at = this.offset;
		this.take(4);
		return this.view.getUint32(at);
	}
	i64() {
		const at = this.offset;
		this.take(8);
		return Number(this.view.getBigInt64(at));
	}
	f64() {
		const at = this.offset;
		this.take(8);
		return this.view.getFloat64(at);
	}
	bytesValue() {
		return this.take(this.u32());
	}
	text() {
		return decoder.decode(this.bytesValue());
	}
	done() {
		if (this.offset !== this.bytes.byteLength) throw this.bindingError("D1_INTERNAL_PROTOCOL_ERROR");
	}
}
function writeValue(writer, value) {
	if (value === null) {
		writer.u8(0);
		return;
	}
	if (typeof value === "number" && Number.isSafeInteger(value)) {
		writer.u8(1);
		writer.i64(value);
		return;
	}
	if (typeof value === "number" && Number.isFinite(value)) {
		writer.u8(2);
		writer.f64(value);
		return;
	}
	if (typeof value === "string") {
		writer.u8(3);
		writer.text(value);
		return;
	}
	if (value instanceof Uint8Array) {
		writer.u8(4);
		writer.bytes(value);
		return;
	}
	throw new TypeError("D1_TYPE_ERROR");
}
function readValue(reader) {
	switch (reader.u8()) {
		case 0: return null;
		case 1: return reader.i64();
		case 2: {
			const value = reader.f64();
			if (!Number.isFinite(value)) throw reader.bindingError("D1_INTERNAL_PROTOCOL_ERROR");
			return value;
		}
		case 3: return reader.text();
		case 4: return Uint8Array.from(reader.bytesValue());
		default: throw reader.bindingError("D1_INTERNAL_PROTOCOL_ERROR");
	}
}
function encodeQuery(mode, statements) {
	const modes = {
		all: 1,
		run: 2,
		raw: 3,
		batch: 4
	};
	const writer = new FrameWriter();
	writer.add(encoder.encode("D1Q1"));
	writer.u8(modes[mode] || 0);
	writer.u16(statements.length);
	for (const statement of statements) {
		writer.text(statement.sql);
		writer.u16(statement.params.length);
		for (const value of statement.params) writeValue(writer, value);
	}
	return writer.finish();
}
function encodeExec(sql) {
	const writer = new FrameWriter();
	writer.add(encoder.encode("D1E1"));
	writer.text(sql);
	return writer.finish();
}
function decodeQuery(buffer, bindingError) {
	const reader = new FrameReader(buffer, bindingError);
	if (decoder.decode(reader.take(4)) !== "D1R1") throw bindingError("D1_INTERNAL_PROTOCOL_ERROR");
	const count = reader.u16();
	const results = [];
	for (let resultIndex = 0; resultIndex < count; resultIndex++) {
		const columnCount = reader.u16();
		const columns = [];
		for (let index = 0; index < columnCount; index++) columns.push(reader.text());
		const rowCount = reader.u32();
		const rows = [];
		for (let rowIndex = 0; rowIndex < rowCount; rowIndex++) {
			const row = [];
			for (let index = 0; index < columnCount; index++) row.push(readValue(reader));
			rows.push(row);
		}
		const meta = JSON.parse(reader.text());
		results.push({
			columns,
			rows,
			meta
		});
	}
	reader.done();
	return { results };
}
export function makeD1TransportBase(bindingError, currentStartupGeneration, tokenHeader) {
	return class extends WorkerEntrypoint {
		#props() {
			const props = this.ctx.props;
			if (!props || typeof props.bindingId !== "string" || typeof props.deploymentId !== "string" || !/^[0-9a-f]{64}$/.test(props.descriptorSha256) || !Number.isSafeInteger(props.resourceSpecGeneration) || props.resourceSpecGeneration < 1) throw bindingError("BINDING_PROTOCOL_ERROR");
			return props;
		}
		async #request(operation, body, permission, contentType) {
			const props = this.#props();
			if (permission === "query") {
				if (!props.permissions.read && !props.permissions.write) {
					throw bindingError("BINDING_PERMISSION_DENIED");
				}
			} else if (!props.permissions[permission]) {
				throw bindingError("BINDING_PERMISSION_DENIED");
			}
			const response = await this.env.BINDING_BACKEND.fetch(`http://binding-backend/internal/bindings/v1/d1/${props.bindingId}/${operation}`, {
				method: "POST",
				headers: {
					"content-type": contentType,
					[tokenHeader]: this.env.BINDING_BACKEND_TOKEN,
					"x-open-compute-startup-generation": currentStartupGeneration(),
					"x-open-compute-deployment-id": props.deploymentId,
					"x-open-compute-descriptor-sha256": props.descriptorSha256,
					"x-open-compute-request-id": crypto.randomUUID()
				},
				body
			});
			if (!response.ok) {
				const code = response.headers.get("x-open-compute-error-code") || "D1_INTERNAL_PROTOCOL_ERROR";
				try {
					await response.body?.cancel();
				} catch {}
				throw bindingError(code);
			}
			return response;
		}
		async query(mode, statements) {
			const response = await this.#request("query", encodeQuery(mode, statements), "query", FRAME_CONTENT_TYPE);
			return decodeQuery(await response.arrayBuffer(), bindingError);
		}
		async exec(sql) {
			const response = await this.#request("exec", encodeExec(sql), "write", FRAME_CONTENT_TYPE);
			if (!response.headers.get("content-type")?.startsWith(JSON_CONTENT_TYPE)) {
				throw bindingError("D1_INTERNAL_PROTOCOL_ERROR");
			}
			return response.json();
		}
		async fetch() {
			throw bindingError("BINDING_PERMISSION_DENIED");
		}
	};
}
