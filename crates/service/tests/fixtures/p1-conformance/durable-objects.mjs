export function checkDurableObjectSurface(namespace) {
  const id = namespace.idFromName("p1-conformance");
  const roundTrip = namespace.idFromString(id.toString());
  return ["idFromName", "newUniqueId", "idFromString", "get", "getByName"].every(
    (method) => typeof namespace[method] === "function",
  ) && roundTrip.toString() === id.toString()
    && typeof namespace.get(id).fetch === "function";
}
