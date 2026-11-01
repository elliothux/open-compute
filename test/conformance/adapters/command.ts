import { execFile } from "node:child_process";
import { promisify } from "node:util";
import { MAX_OUTPUT } from "./runtime-contract.ts";
import { cloudflareTransientFailure } from "./transport.ts";
import type { CommandResult } from "./types.ts";

const executeFile = promisify(execFile);

export async function command(
  executable: string,
  args: readonly string[],
  options: {
    readonly cwd: string;
    readonly env: Readonly<Record<string, string>>;
    readonly timeout: number;
  },
): Promise<{ stdout: string; stderr: string }> {
  let result = await commandStatus(executable, args, options);
  // The Cloudflare API drops connections now and then. A repeated create fails
  // visibly on the existing resource, so a bounded retry cannot hide a defect.
  for (
    let attempt = 1;
    attempt < 3 &&
    result.status !== 0 &&
    cloudflareTransientFailure(`${result.stdout}\n${result.stderr}`);
    attempt++
  ) {
    await new Promise((resolve) => setTimeout(resolve, 2_000));
    result = await commandStatus(executable, args, options);
  }
  if (result.status !== 0) {
    throw new Error(
      `external command failed; stdout=${result.stdout.slice(0, 512)}; stderr=${result.stderr.slice(0, 512)}`,
    );
  }
  return result;
}

export async function commandStatus(
  executable: string,
  args: readonly string[],
  options: {
    readonly cwd: string;
    readonly env: Readonly<Record<string, string>>;
    readonly timeout: number;
  },
): Promise<CommandResult> {
  try {
    const result = await executeFile(executable, [...args], {
      cwd: options.cwd,
      env: options.env,
      timeout: options.timeout,
      maxBuffer: MAX_OUTPUT,
      encoding: "utf8",
    });
    return { status: 0, stdout: result.stdout, stderr: result.stderr };
  } catch (error) {
    if (error !== null && typeof error === "object") {
      const stderr: unknown = Reflect.get(error, "stderr");
      const stdout: unknown = Reflect.get(error, "stdout");
      const code: unknown = Reflect.get(error, "code");
      return {
        status: typeof code === "number" ? code : -1,
        stdout: typeof stdout === "string" ? stdout : "",
        stderr: typeof stderr === "string" ? stderr : "",
      };
    }
    return { status: -1, stdout: "", stderr: "" };
  }
}
