import type { AssetHeaderRule, AssetRedirectRule } from "./types.ts";

const HEADER_NAME = /^[!#$%&'*+.^_`|~0-9A-Za-z-]+$/;
const REDIRECT_STATUS = new Set([200, 301, 302, 303, 307, 308]);

function lines(content: string, label: string): string[] {
  if (content.includes("\0")) throw new Error(`${label} contains NUL`);
  return content.replaceAll("\r\n", "\n").split("\n").map((line, index) => {
    if (line.length > 2_000) throw new Error(`${label} line ${index + 1} exceeds 2000 characters`);
    return line;
  });
}

function pattern(value: string, label: string, line: number): string {
  if ((!value.startsWith("/") && !value.startsWith("https://")) || value.includes("\\")
      || [...value.matchAll(/\*/g)].length > 1) {
    throw new Error(`${label} line ${line} has an invalid pattern`);
  }
  for (const token of value.matchAll(/:/g)) {
    const suffix = value.slice((token.index ?? 0) + 1);
    if (suffix.startsWith("//")) continue;
    if (!/^[A-Za-z][A-Za-z0-9_]*/.test(suffix)) {
      throw new Error(`${label} line ${line} has an invalid placeholder`);
    }
  }
  return value;
}

/** Parse `_headers` strictly; malformed lines fail the deployment with a stable line number. */
export function parseHeaders(content: string): AssetHeaderRule[] {
  const input = lines(content, "_headers");
  const result: AssetHeaderRule[] = [];
  let current: { pattern: string; operations: { name: string; value: string | null }[] } | undefined;
  for (let index = 0; index < input.length; index += 1) {
    const raw = input[index]!;
    if (!raw.trim() || raw.trimStart().startsWith("#")) continue;
    if (!/^\s/.test(raw)) {
      current = { pattern: pattern(raw.trim(), "_headers", index + 1), operations: [] };
      result.push(current);
      if (result.length > 100) throw new Error("_headers exceeds 100 rules");
      continue;
    }
    if (!current) throw new Error(`_headers line ${index + 1} has no rule pattern`);
    const operation = raw.trim();
    if (operation.startsWith("! ")) {
      const name = operation.slice(2).trim().toLowerCase();
      if (!HEADER_NAME.test(name)) throw new Error(`_headers line ${index + 1} has an invalid header name`);
      current.operations.push({ name, value: null });
      continue;
    }
    const separator = operation.indexOf(":");
    if (separator < 1) throw new Error(`_headers line ${index + 1} has an invalid header operation`);
    const name = operation.slice(0, separator).trim().toLowerCase();
    const value = operation.slice(separator + 1).trim();
    if (!HEADER_NAME.test(name) || /[\r\n\0]/.test(value)) {
      throw new Error(`_headers line ${index + 1} has an invalid header operation`);
    }
    current.operations.push({ name, value });
  }
  for (const [index, rule] of result.entries()) {
    if (!rule.operations.length || rule.operations.length > 100) {
      throw new Error(`_headers rule ${index + 1} has an invalid operation count`);
    }
    const names = new Set<string>();
    for (const operation of rule.operations) {
      if (names.has(operation.name)) throw new Error(`_headers rule ${index + 1} repeats a header name`);
      names.add(operation.name);
    }
  }
  return result;
}

/** Parse `_redirects` strictly, including same-origin `200` rewrites. */
export function parseRedirects(content: string): AssetRedirectRule[] {
  const result: AssetRedirectRule[] = [];
  for (const [index, raw] of lines(content, "_redirects").entries()) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const fields = line.split(/\s+/);
    if (fields.length < 2 || fields.length > 3) throw new Error(`_redirects line ${index + 1} is invalid`);
    const from = pattern(fields[0]!, "_redirects", index + 1);
    const to = fields[1]!;
    const status = fields.length === 2 ? 302 : Number(fields[2]);
    if (!REDIRECT_STATUS.has(status) || !to || /[\r\n\0]/.test(to)
        || (status === 200 && !to.startsWith("/"))) {
      throw new Error(`_redirects line ${index + 1} is invalid`);
    }
    result.push({ from, to, status: status as AssetRedirectRule["status"] });
    if (result.length > 2_000) throw new Error("_redirects exceeds 2000 rules");
  }
  return result;
}
