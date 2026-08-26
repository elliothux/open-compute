export function checkAdversarialValues() {
  const nullPrototype = Object.create(null);
  Object.defineProperty(nullPrototype, "value", { get: () => 7, enumerable: true });
  const proxy = new Proxy(nullPrototype, {
    get(target, property, receiver) {
      return Reflect.get(target, property, receiver);
    },
  });
  const cyclic = { ok: true };
  cyclic.self = cyclic;
  const cloned = structuredClone(cyclic);
  const parsed = JSON.parse('{"__proto__":{"polluted":true},"constructor":{"prototype":{}}}');
  return proxy.value === 7
    && cloned.self === cloned
    && parsed.__proto__.polluted === true
    && Object.prototype.polluted === undefined
    && typeof Symbol("p1") === "symbol";
}
