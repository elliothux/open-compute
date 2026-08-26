export function checkKvSurface(namespace) {
  const methods = ["get", "getWithMetadata", "put", "delete", "list"];
  return methods.every((method) => typeof namespace[method] === "function")
    && !Object.prototype.hasOwnProperty.call(namespace, "__openComputeInternal");
}
