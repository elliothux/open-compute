import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { runCli } from "../src/cli.ts";

test("oc remains an offline build and type-generation tool", async () => {
  await assert.rejects(runCli(["deploy"]), /Usage: oc <build\|types>/);
  const source = await readFile(
    new URL("../src/cli.ts", import.meta.url),
    "utf8",
  );
  for (const removed of [
    "wranglerEntrypoint",
    "wranglerArgs",
    "node:child_process",
    "CLOUDFLARE_API_TOKEN",
    "deploymentPackage",
    "contentKind",
    "bytesBase64",
    "manifest: assets",
    "routing: assets",
  ]) {
    assert.equal(
      source.includes(removed),
      false,
      `online or removed private transport token remains: ${removed}`,
    );
  }
});
