// Generated from runtime/src/durable-objects/id-codec.ts by Rolldown. Do not edit.
const Encoder = TextEncoder;
const encoder = new Encoder();
const randomValues = crypto.getRandomValues.bind(crypto);
const decodeBase64 = atob.bind(globalThis);
const Bytes = Uint8Array;
const Words = Uint32Array;
const View = DataView;
const ceil = Math.ceil;
const floor = Math.floor;
const HEX = "0123456789abcdef";
const K = new Words([
	1116352408,
	1899447441,
	3049323471,
	3921009573,
	961987163,
	1508970993,
	2453635748,
	2870763221,
	3624381080,
	310598401,
	607225278,
	1426881987,
	1925078388,
	2162078206,
	2614888103,
	3248222580,
	3835390401,
	4022224774,
	264347078,
	604807628,
	770255983,
	1249150122,
	1555081692,
	1996064986,
	2554220882,
	2821834349,
	2952996808,
	3210313671,
	3336571891,
	3584528711,
	113926993,
	338241895,
	666307205,
	773529912,
	1294757372,
	1396182291,
	1695183700,
	1986661051,
	2177026350,
	2456956037,
	2730485921,
	2820302411,
	3259730800,
	3345764771,
	3516065817,
	3600352804,
	4094571909,
	275423344,
	430227734,
	506948616,
	659060556,
	883997877,
	958139571,
	1322822218,
	1537002063,
	1747873779,
	1955562222,
	2024104815,
	2227730452,
	2361852424,
	2428436474,
	2756734187,
	3204031479,
	3329325298
]);
function rotr(value, bits) {
	return value >>> bits | value << 32 - bits;
}
export function sha256(input) {
	const bytes = input instanceof Bytes ? input : new Bytes(input);
	const bitLength = bytes.length * 8;
	const paddedLength = ceil((bytes.length + 9) / 64) * 64;
	const padded = new Bytes(paddedLength);
	padded.set(bytes);
	padded[bytes.length] = 128;
	const view = new View(padded.buffer);
	view.setUint32(paddedLength - 8, floor(bitLength / 4294967296));
	view.setUint32(paddedLength - 4, bitLength >>> 0);
	const h = new Words([
		1779033703,
		3144134277,
		1013904242,
		2773480762,
		1359893119,
		2600822924,
		528734635,
		1541459225
	]);
	const w = new Words(64);
	for (let offset = 0; offset < paddedLength; offset += 64) {
		for (let i = 0; i < 16; i++) w[i] = view.getUint32(offset + i * 4);
		for (let i = 16; i < 64; i++) {
			const s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ w[i - 15] >>> 3;
			const s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ w[i - 2] >>> 10;
			w[i] = w[i - 16] + s0 + w[i - 7] + s1 >>> 0;
		}
		let [a, b, c, d, e, f, g, hh] = [
			h[0],
			h[1],
			h[2],
			h[3],
			h[4],
			h[5],
			h[6],
			h[7]
		];
		for (let i = 0; i < 64; i++) {
			const s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25);
			const ch = e & f ^ ~e & g;
			const t1 = hh + s1 + ch + K[i] + w[i] >>> 0;
			const s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22);
			const maj = a & b ^ a & c ^ b & c;
			const t2 = s0 + maj >>> 0;
			hh = g;
			g = f;
			f = e;
			e = d + t1 >>> 0;
			d = c;
			c = b;
			b = a;
			a = t1 + t2 >>> 0;
		}
		h[0] = h[0] + a >>> 0;
		h[1] = h[1] + b >>> 0;
		h[2] = h[2] + c >>> 0;
		h[3] = h[3] + d >>> 0;
		h[4] = h[4] + e >>> 0;
		h[5] = h[5] + f >>> 0;
		h[6] = h[6] + g >>> 0;
		h[7] = h[7] + hh >>> 0;
	}
	const output = new Bytes(32);
	const outputView = new View(output.buffer);
	for (let i = 0; i < h.length; i++) outputView.setUint32(i * 4, h[i]);
	return output;
}
export function hmacSha256(key, message) {
	let material = key instanceof Bytes ? key : new Bytes(key);
	if (material.length > 64) material = sha256(material);
	const inner = new Bytes(64 + message.length);
	const outer = new Bytes(64 + 32);
	inner.fill(54, 0, 64);
	outer.fill(92, 0, 64);
	for (let i = 0; i < material.length; i++) {
		inner[i] = inner[i] ^ material[i];
		outer[i] = outer[i] ^ material[i];
	}
	inner.set(message, 64);
	outer.set(sha256(inner), 64);
	return sha256(outer);
}
export function utf8(value) {
	return encoder.encode(value);
}
export function base64Bytes(value) {
	const binary = decodeBase64(value.replace(/-/g, "+").replace(/_/g, "/"));
	const output = new Bytes(binary.length);
	for (let i = 0; i < binary.length; i++) output[i] = binary.charCodeAt(i);
	return output;
}
export function hex(bytes) {
	let value = "";
	for (const byte of bytes) value += HEX[byte >>> 4] + HEX[byte & 15];
	return value;
}
export function randomBytes(length) {
	const output = new Bytes(length);
	randomValues(output);
	return output;
}
