// Explicit build/test input acquisition; never shipped or invoked by platformd.
import { mkdir, rm } from "node:fs/promises";
import { join } from "node:path";
import { absoluteDestination, prepareWorkerd, sourceArguments } from "./workerd-archive.ts";

const input = sourceArguments(process.argv.slice(2));
const destination = await absoluteDestination(input.destination);
await mkdir(destination, { mode: 0o700 }); // Exclusive creation, including at publication races.
let complete = false;
try {
  const result = await prepareWorkerd(destination, input.archive, input.download);
  complete = true;
  console.log(`OPEN_COMPUTE_BUILD_WORKERD_ARCHIVE=${join(destination, result.archiveName)}`);
  console.log(`OPEN_COMPUTE_TEST_WORKERD=${join(destination, "workerd")}`);
} finally {
  if (!complete) await rm(destination, { recursive: true, force: true });
}
