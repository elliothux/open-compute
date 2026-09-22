/** Keep platform capability state private when tenant code edits built-in prototypes. */
const NativeWeakMap = WeakMap;
const apply = Reflect.apply;
const define = Object.defineProperties;
const get = WeakMap.prototype.get;
const set = WeakMap.prototype.set;
const has = WeakMap.prototype.has;
const remove = WeakMap.prototype.delete;

export function privateWeakMap<K extends object, V>(): WeakMap<K, V> {
  const map = new NativeWeakMap<K, V>();
  define(map, {
    get: { value: (key: K): V | undefined => apply(get, map, [key]) },
    set: {
      value: (key: K, value: V): WeakMap<K, V> => {
        apply(set, map, [key, value]);
        return map;
      },
    },
    has: { value: (key: K): boolean => apply(has, map, [key]) },
    delete: { value: (key: K): boolean => apply(remove, map, [key]) },
  });
  return map;
}
