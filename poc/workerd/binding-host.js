import { WorkerEntrypoint } from "cloudflare:workers";
import { logEvent, sha256hex } from "./log.js";

const KNOWN_RESOURCES = Object.freeze(["kv_fixture_a", "kv_fixture_b"]);
const INITIAL = Object.freeze({
  kv_fixture_a: Object.freeze([["shared", "A"]]),
  kv_fixture_b: Object.freeze([["shared", "B"]]),
});

const stores = new Map(
  KNOWN_RESOURCES.map((id) => [id, new Map(INITIAL[id])])
);

let faults = {};

function isFault(point, resourceId) {
  const value = faults[point];
  if (!value) return false;
  if (value === true) return true;
  return value === resourceId;
}

function knownStore(resourceId) {
  if (typeof resourceId !== "string" || !stores.has(resourceId)) {
    const err = new Error("internal adapter failure secret=g0-master-key path=/var/g0-data/do/secret.sqlite");
    err.stack =
      "Error: internal adapter failure secret=g0-master-key\n    at BindingBackend (/Users/g0/secret.js:1:1)";
    throw err;
  }
  return stores.get(resourceId);
}

function assertString(name, value) {
  if (typeof value !== "string") {
    throw new TypeError(`${name} must be a string`);
  }
}

function injectInternalError() {
  const err = new Error(
    "internal adapter failure secret=g0-master-key path=/var/g0-data/do/secret.sqlite"
  );
  err.stack =
    "Error: internal adapter failure secret=g0-master-key\n    at BindingBackend.get (/Users/g0/secret.js:1:1)";
  throw err;
}

export class BindingBackend extends WorkerEntrypoint {
  async get(resourceId, key) {
    if (isFault("F4", resourceId)) {
      throw new Error("FAULT_INJECTED:F4");
    }
    if (key === "__g0_internal_error") {
      injectInternalError();
    }
    assertString("key", key);
    const value = knownStore(resourceId).get(key) ?? null;
    if (isFault("F5", resourceId)) {
      throw new Error("FAULT_INJECTED:F5");
    }
    logEvent({
      bindingType: "FixtureKV",
      resourceIdHash: await sha256hex(resourceId),
      outcome: "ok",
      extra: { op: "get" },
    });
    return value;
  }

  async put(resourceId, key, value) {
    if (isFault("F4", resourceId)) {
      throw new Error("FAULT_INJECTED:F4");
    }
    assertString("key", key);
    assertString("value", value);
    knownStore(resourceId).set(key, value);
    if (isFault("F5", resourceId)) {
      throw new Error("FAULT_INJECTED:F5");
    }
    logEvent({
      bindingType: "FixtureKV",
      resourceIdHash: await sha256hex(resourceId),
      outcome: "ok",
      extra: { op: "put" },
    });
  }
}

export default {
  async fetch(request) {
    const url = new URL(request.url);
    if (url.pathname === "/fault") {
      const body = await request.json();
      if (!body.enabled) {
        faults[body.point] = false;
      } else if (typeof body.resourceId === "string" && body.resourceId) {
        faults[body.point] = body.resourceId;
      } else {
        faults[body.point] = true;
      }
      return Response.json({ ok: true, faults });
    }
    return Response.json({ ok: false, errorCode: "NOT_PUBLIC" }, { status: 404 });
  },
};
