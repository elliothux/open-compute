import assert from "node:assert/strict";
import test from "node:test";
import { codec, decode, encode, format } from "./load.mjs";

function header(profile = "workflow") {
  return Uint8Array.of(
    ...codec.DURABLE_VALUE_MAGIC,
    codec.DURABLE_VALUE_SCHEMA,
    codec.DURABLE_VALUE_PROFILE_ID[profile],
  );
}

function concat(...parts) {
  const bytes = new Uint8Array(parts.reduce((sum, part) => sum + part.byteLength, 0));
  let offset = 0;
  for (const part of parts) {
    bytes.set(part, offset);
    offset += part.byteLength;
  }
  return bytes;
}

function u32(value) {
  const bytes = new Uint8Array(4);
  new DataView(bytes.buffer).setUint32(0, value);
  return bytes;
}

function str(value) {
  const encoded = new TextEncoder().encode(value);
  return concat(u32(encoded.byteLength), encoded);
}

function malformed(bytes, profile = "workflow") {
  const code = profile === "queue-v8" ? "QUEUE_V8_MALFORMED" : "WORKFLOW_SERIALIZATION_MALFORMED";
  assert.throws(() => decode(bytes, profile), { message: code });
}

test("magic, schema, profile, truncation, and trailing bytes fail closed", () => {
  const valid = encode({ a: 1 }, "workflow");
  malformed(valid.subarray(0, 0));
  malformed(valid.subarray(0, 5));
  malformed(valid.subarray(0, valid.byteLength - 1));
  malformed(concat(valid, Uint8Array.of(0)));
  const badMagic = Uint8Array.from(valid);
  badMagic[0] ^= 1;
  malformed(badMagic);
  const badSchema = Uint8Array.from(valid);
  badSchema[4] = 2;
  malformed(badSchema);
  malformed(valid, "queue-v8");
  malformed(encode(1, "queue-v8"), "workflow");
  assert.deepEqual(decode(new Uint8Array(valid.buffer, valid.byteOffset, valid.byteLength), "workflow"), { a: 1 });
  assert.throws(() => decode(valid.buffer, "workflow"), { message: "WORKFLOW_SERIALIZATION_MALFORMED" });
  assert.throws(() => decode("OCDV", "workflow"), { message: "WORKFLOW_SERIALIZATION_MALFORMED" });
});

test("unknown tags, bad references, duplicate keys, and invalid lengths are rejected", () => {
  malformed(concat(header(), Uint8Array.of(0xff)));
  malformed(concat(header(), Uint8Array.of(format.TAG.HOLE)));
  malformed(concat(header(), Uint8Array.of(format.TAG.REF), u32(0)));
  malformed(concat(header(), Uint8Array.of(format.TAG.REF), u32(99)));
  malformed(concat(
    header(), Uint8Array.of(format.TAG.OBJECT), u32(2), str("a"), Uint8Array.of(format.TAG.TRUE),
    str("a"), Uint8Array.of(format.TAG.FALSE),
  ));
  malformed(concat(header(), Uint8Array.of(format.TAG.ARRAY), u32(1), u32(0)));
  malformed(concat(header(), Uint8Array.of(format.TAG.STRING), u32(8), Uint8Array.of(1, 2)));
  malformed(concat(header(), Uint8Array.of(format.TAG.BIGINT), Uint8Array.of(2), u32(0)));
  malformed(concat(header(), Uint8Array.of(format.TAG.BIGINT), Uint8Array.of(0), u32(1), Uint8Array.of(0)));
  malformed(concat(header(), Uint8Array.of(format.TAG.TYPED_ARRAY), Uint8Array.of(99), Uint8Array.of(format.TAG.NULL)));
  malformed(concat(
    header(), Uint8Array.of(format.TAG.DATA_VIEW), Uint8Array.of(format.TAG.TRUE), u32(0), u32(0),
  ));
  const buffer = concat(
    header(), Uint8Array.of(format.TAG.TYPED_ARRAY), Uint8Array.of(1),
    Uint8Array.of(format.TAG.ARRAY_BUFFER), u32(1), Uint8Array.of(9), u32(0), u32(4),
  );
  malformed(buffer);
  malformed(concat(header(), Uint8Array.of(format.TAG.REGEXP), str("("), str("g")));
  malformed(concat(header(), Uint8Array.of(format.TAG.ERROR), Uint8Array.of(99), str("E"), str("m"), Uint8Array.of(0)));
  const overlong = concat(header(), Uint8Array.of(format.TAG.STRING), u32(2), Uint8Array.of(0xc0, 0x80));
  malformed(overlong);
  const surrogateFour = concat(
    header(), Uint8Array.of(format.TAG.STRING), u32(4), Uint8Array.of(0xf0, 0x80, 0x80, 0x80),
  );
  malformed(surrogateFour);
});

test("decode does not execute payload text or repair trailing/truncated input", () => {
  const payload = "throw new Error('executed')";
  const bytes = encode(payload, "workflow");
  assert.equal(decode(bytes, "workflow"), payload);
  malformed(concat(bytes, new TextEncoder().encode(";throw 1")));
  const object = concat(
    header(), Uint8Array.of(format.TAG.OBJECT), u32(1), str("x"), Uint8Array.of(format.TAG.TRUE), Uint8Array.of(0),
  );
  malformed(object);
});
