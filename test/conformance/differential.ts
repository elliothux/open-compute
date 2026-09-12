import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { loadPortableFixtures } from "./adapters/fixtures.ts";
import { runDifferential } from "./differential/runner.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const fixtures = await loadPortableFixtures(
  join(ROOT, "test/conformance/fixtures"),
);
const args = process.argv.slice(2);

if (args.length === 1 && args[0] === "--list") {
  process.stdout.write(
    `${JSON.stringify({ schemaVersion: 1, cases: fixtures.map((fixture) => fixture.id) })}\n`,
  );
} else {
  const selected: string[] = [];
  for (let index = 0; index < args.length; index += 2) {
    if (args[index] !== "--case" || args[index + 1] === undefined)
      throw new Error("use --case <id>");
    selected.push(args[index + 1]!);
  }
  const requested = selected.length
    ? selected
    : fixtures.map((fixture) => fixture.id);
  const selectedFixtures = requested.map((id) => {
    const fixture = fixtures.find((item) => item.id === id);
    if (fixture === undefined)
      throw new Error(`unknown differential fixture: ${id}`);
    return fixture;
  });
  if (new Set(requested).size !== requested.length)
    throw new Error("duplicate differential fixture");
  const result = await runDifferential(ROOT, selectedFixtures);
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (result.status !== "passed") process.exitCode = 1;
}
