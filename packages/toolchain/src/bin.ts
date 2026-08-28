#!/usr/bin/env node
import { runCli } from "./cli.ts";

try { await runCli(process.argv.slice(2)); }
catch (error) {
  process.stderr.write(`${error instanceof Error ? error.message : "Worker command failed"}\n`);
  process.exitCode = 1;
}
