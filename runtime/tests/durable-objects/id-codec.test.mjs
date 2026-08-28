import assert from "node:assert/strict";
import { createHash, createHmac, randomBytes } from "node:crypto";
import test from "node:test";
import { importRuntime } from "../compiled-runtime.mjs";

const codec = await importRuntime("durable-objects/id-codec.ts");

test("compiled synchronous ID codec matches native SHA-256 and HMAC at block boundaries", () => {
  for (const length of [0, 1, 55, 56, 63, 64, 65, 127, 128, 1024]) {
    const input = randomBytes(length);
    assert.equal(codec.hex(codec.sha256(input)), createHash("sha256").update(input).digest("hex"));
    for (const keyLength of [0, 32, 64, 65, 128]) {
      const key = randomBytes(keyLength);
      assert.equal(codec.hex(codec.hmacSha256(key, input)), createHmac("sha256", key).update(input).digest("hex"));
    }
  }
  const bytes = codec.utf8("命名对象");
  assert.deepEqual(codec.base64Bytes(Buffer.from(bytes).toString("base64url")), bytes);
});
