"use strict";

class AssertError extends Error {
  constructor(message, extra) {
    super(message);
    this.name = "AssertError";
    this.extra = extra || null;
  }
}

function inspect(value) {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function equal(actual, expected, message) {
  if (actual !== expected) {
    throw new AssertError(`${message}: expected ${inspect(expected)}, got ${inspect(actual)}`);
  }
}

function deepEqual(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new AssertError(`${message}: expected ${inspect(expected)}, got ${inspect(actual)}`);
  }
}

function isTrue(value, message) {
  if (!value) throw new AssertError(message);
}

function isFalse(value, message) {
  if (value) throw new AssertError(message);
}

function includes(haystack, needle, message) {
  const text = typeof haystack === "string" ? haystack : inspect(haystack);
  if (!text.includes(needle)) {
    throw new AssertError(`${message}: expected to include ${inspect(needle)}`);
  }
}

function excludes(haystack, needle, message) {
  const text = haystack == null ? "" : typeof haystack === "string" ? haystack : inspect(haystack);
  if (text.includes(needle)) {
    throw new AssertError(`${message}: expected not to include ${inspect(needle)}`);
  }
}

function okStatus(res, message) {
  if (!res.ok) {
    throw new AssertError(
      `${message}: HTTP ${res.status} ${inspect(res.json || res.text)}`
    );
  }
}

function match(value, regex, message) {
  if (!regex.test(String(value))) {
    throw new AssertError(`${message}: ${inspect(value)} did not match ${regex}`);
  }
}

async function rejects(fn, needle, message) {
  try {
    await fn();
  } catch (err) {
    const text = String(err && err.message ? err.message : err);
    if (needle instanceof RegExp) {
      if (!needle.test(text)) {
        throw new AssertError(`${message}: thrown ${inspect(text)} did not match ${needle}`);
      }
    } else if (needle && !text.includes(needle)) {
      throw new AssertError(`${message}: thrown ${inspect(text)} did not include ${inspect(needle)}`);
    }
    return err;
  }
  throw new AssertError(`${message}: expected to throw`);
}

module.exports = {
  AssertError,
  equal,
  deepEqual,
  isTrue,
  isFalse,
  includes,
  excludes,
  okStatus,
  match,
  rejects,
};
