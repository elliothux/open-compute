import { COMPAT_DATE, getDeployment } from "./registry.js";
import { sha256hex } from "./log.js";

const seenHashes = new Map();

export function resetCodeCache() {
  seenHashes.clear();
}

export async function readFixtureFile(env, root, file) {
  const resp = await env.FIXTURES.fetch(`http://g0-fixtures/${root}/${file}`);
  if (!resp.ok) {
    throw new Error("FIXTURE_NOT_FOUND");
  }
  return new TextDecoder().decode(await resp.arrayBuffer());
}

function specFor(key, options = {}) {
  const spec = options.specOverride || getDeployment(key);
  if (!spec) {
    const err = new Error("DEPLOYMENT_NOT_FOUND");
    err.errorCode = "DEPLOYMENT_NOT_FOUND";
    throw err;
  }
  if (options.alternateRoot) {
    return { ...spec, root: options.alternateRoot };
  }
  return spec;
}

export async function fingerprintWorkerCode(env, key, options = {}) {
  const spec = specFor(key, options);
  const modules = {};
  for (const file of spec.files) {
    modules[file] = await readFixtureFile(env, spec.root, file);
  }
  const fingerprint = JSON.stringify({
    key,
    compatibilityDate: COMPAT_DATE,
    mainModule: spec.mainModule,
    modules,
    globalOutbound: null,
    resourceId: spec.resourceId ?? null,
    kind: spec.kind,
  });
  return { spec, modules, hash: await sha256hex(fingerprint) };
}

export function assertImmutableHash(key, hash) {
  const prev = seenHashes.get(key);
  if (prev && prev !== hash) {
    const err = new Error("PLATFORM_INVARIANT_VIOLATION");
    err.errorCode = "PLATFORM_INVARIANT_VIOLATION";
    throw err;
  }
}

export function rememberHash(key, hash) {
  assertImmutableHash(key, hash);
  seenHashes.set(key, hash);
}

export async function assembleWorkerCode(env, key, options = {}) {
  const { spec, modules, hash } = await fingerprintWorkerCode(env, key, options);
  assertImmutableHash(key, hash);
  if (!options.dryRun) {
    seenHashes.set(key, hash);
  }

  const envBindings = { ...(options.extraEnv ?? {}) };
  if (spec.kind === "binding") {
    if (!options.kvStub) {
      throw new Error("BINDING_HOST_UNAVAILABLE");
    }
    envBindings.KV = options.kvStub;
  }

  const code = {
    compatibilityDate: COMPAT_DATE,
    mainModule: spec.mainModule,
    modules,
    env: envBindings,
    globalOutbound: null,
  };
  if (spec.kind === "binding" || spec.kind === "do") {
    code.compatibilityFlags = ["rpc"];
  }

  return {
    spec,
    hash,
    code,
  };
}

export function rememberOverride(key, hash) {
  rememberHash(key, hash);
}

export function getSeenHash(key) {
  return seenHashes.get(key) ?? null;
}
