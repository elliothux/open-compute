import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

interface Schema {
  $ref?: string;
  const?: string | number | boolean;
  type?: string;
  enum?: string[];
  required?: string[];
  properties?: Record<string, Schema>;
  items?: Schema;
  additionalProperties?: Schema | boolean;
}

interface Contract {
  paths: Record<string, Record<string, { operationId: string }>>;
  components: { schemas: Record<string, Schema> };
}

const root = resolve(import.meta.dir, "..");
const schemaPath = resolve(root, "../../openapi/open-compute-extension.json");
const outputPath = resolve(root, "src/generated.ts");
const runtimeOutputPath = resolve(root, "src/generated.js");
const bytes = await readFile(schemaPath);
const contract = JSON.parse(bytes.toString("utf8")) as Contract;

function typeFor(schema: Schema | boolean, indent = ""): string {
  if (schema === true) return "unknown";
  if (schema === false) return "never";
  if (schema.$ref !== undefined) return schema.$ref.split("/").at(-1) ?? "unknown";
  if (schema.const !== undefined) return JSON.stringify(schema.const);
  if (schema.enum !== undefined) return schema.enum.map(value => JSON.stringify(value)).join(" | ");
  if (schema.type === "string") return "string";
  if (schema.type === "integer" || schema.type === "number") return "number";
  if (schema.type === "boolean") return "boolean";
  if (schema.type === "null") return "null";
  if (schema.type === "array") return `readonly ${typeFor(schema.items ?? {}, indent)}[]`;
  if (schema.type === "object") {
    const required = new Set(schema.required ?? []);
    const entries = Object.entries(schema.properties ?? {});
    if (entries.length === 0 && schema.additionalProperties !== undefined) {
      return `Record<string, ${typeFor(schema.additionalProperties, indent)}>`;
    }
    if (entries.length === 0) return "Record<string, never>";
    const next = `${indent}  `;
    return `{\n${entries.map(([name, value]) => `${next}readonly ${name}${required.has(name) ? "" : "?"}: ${typeFor(value, next)};`).join("\n")}\n${indent}}`;
  }
  if (Object.keys(schema).length === 0) return "unknown";
  throw new Error(`unsupported extension schema node ${JSON.stringify(schema)}`);
}

const operations = Object.entries(contract.paths).flatMap(([path, methods]) =>
  Object.entries(methods).map(([method, operation]) => ({ method: method.toUpperCase(), path, operationId: operation.operationId })),
).sort((left, right) => left.operationId.localeCompare(right.operationId));
const digest = createHash("sha256").update(bytes).digest("hex");
const declarations = Object.entries(contract.components.schemas).map(([name, schema]) =>
  `export type ${name} = ${typeFor(schema)};`,
).join("\n\n");
const output = `// Generated from ../../openapi/open-compute-extension.json. Do not edit.\n` +
  `export const OPEN_COMPUTE_EXTENSION_SCHEMA_SHA256 = ${JSON.stringify(digest)};\n\n` +
  `export const OPEN_COMPUTE_EXTENSION_OPERATIONS = ${JSON.stringify(operations, null, 2)} as const;\n\n` +
  `${declarations}\n`;
const runtimeOutput = `// Generated from ../../openapi/open-compute-extension.json. Do not edit.\n` +
  `export const OPEN_COMPUTE_EXTENSION_SCHEMA_SHA256 = ${JSON.stringify(digest)};\n\n` +
  `export const OPEN_COMPUTE_EXTENSION_OPERATIONS = ${JSON.stringify(operations, null, 2)};\n`;

if (process.argv.includes("--check")) {
  const current = await readFile(outputPath, "utf8").catch(() => "");
  if (current !== output) throw new Error("generated extension types are stale; run bun run generate");
  const currentRuntime = await readFile(runtimeOutputPath, "utf8").catch(() => "");
  if (currentRuntime !== runtimeOutput) throw new Error("generated extension runtime is stale; run bun run generate");
} else {
  await writeFile(outputPath, output);
  await writeFile(runtimeOutputPath, runtimeOutput);
}
