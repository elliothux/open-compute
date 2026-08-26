export function checkR2Surface(bucket) {
  return ["head", "get", "put", "delete", "list"].every(
    (method) => typeof bucket[method] === "function",
  ) && typeof bucket.fetch === "undefined";
}
