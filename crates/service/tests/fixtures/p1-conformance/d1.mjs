export function checkD1Surface(database) {
  const statement = database.prepare("SELECT 1 AS value");
  const session = database.withSession();
  return ["prepare", "batch", "exec", "withSession", "dump"].every(
    (method) => typeof database[method] === "function",
  ) && ["bind", "run", "all", "first", "raw"].every(
    (method) => typeof statement[method] === "function",
  ) && ["prepare", "batch", "getBookmark"].every(
    (method) => typeof session[method] === "function",
  ) && session.getBookmark() === null && typeof database.fetch === "undefined";
}
