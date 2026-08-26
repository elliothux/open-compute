export async function checkMaliciousWorkerSurface(env) {
  const forbiddenBindings = [
    "BINDING_BACKEND",
    "RUNTIME_SOURCE",
    "SYSTEM_S3",
    "SQLITE",
    "INTERNAL_FETCHER",
  ];
  if (forbiddenBindings.some((name) => Reflect.has(env, name))) return false;

  const inheritedBefore = Object.prototype.p1Polluted;
  const attackerObject = JSON.parse(
    '{"__proto__":{"p1Polluted":true},"constructor":{"prototype":{"p1Polluted":true}}}',
  );
  if (inheritedBefore !== undefined || Object.prototype.p1Polluted !== undefined) return false;
  if (attackerObject.__proto__.p1Polluted !== true) return false;

  let getterRuns = 0;
  const getter = Object.create(null);
  Object.defineProperty(getter, "value", {
    enumerable: true,
    get() {
      getterRuns += 1;
      return "bounded";
    },
  });
  const proxy = new Proxy(getter, {
    get(target, property, receiver) {
      if (typeof property === "symbol") return Reflect.get(target, property, receiver);
      return Reflect.get(target, property, receiver);
    },
  });
  if (proxy.value !== "bounded" || getterRuns !== 1) return false;

  const cyclic = { value: "cycle" };
  cyclic.self = cyclic;
  const cloned = structuredClone(cyclic);
  if (cloned.self !== cloned || cloned.value !== "cycle") return false;

  let toJsonFailedClosed = false;
  try {
    JSON.stringify({
      toJSON() {
        throw new Error("p1-to-json-trap");
      },
    });
  } catch (error) {
    toJsonFailedClosed = String(error.message).includes("p1-to-json-trap");
  }
  if (!toJsonFailedClosed) return false;

  let cancelled = false;
  const reader = new ReadableStream({
    pull(controller) {
      controller.enqueue(new Uint8Array([1]));
    },
    cancel() {
      cancelled = true;
    },
  }).getReader();
  await reader.read();
  await reader.cancel("p1-cancel");
  if (!cancelled) return false;

  const spoofed = new Headers([
    ["x-open-compute-account-id", "attacker"],
    ["x-open-compute-binding-token", "attacker"],
    ["connection", "keep-alive"],
  ]);
  return spoofed.get("x-open-compute-account-id") === "attacker"
    && typeof env.CACHE.get === "function"
    && typeof env.BUCKET.get === "function"
    && typeof env.DB.prepare === "function"
    && typeof env.OBJECTS.getByName === "function"
    && Object.prototype.p1Polluted === undefined;
}
