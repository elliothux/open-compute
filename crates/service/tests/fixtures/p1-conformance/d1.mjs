export function checkD1Surface(database) {
  const statement = database.prepare("SELECT 1 AS value");
  return ["prepare", "batch", "exec", "withSession"].every(
    (method) => typeof database[method] === "function",
  ) && ["bind", "run", "all", "first", "raw"].every(
    (method) => typeof statement[method] === "function",
  ) && typeof database.fetch === "undefined";
}
