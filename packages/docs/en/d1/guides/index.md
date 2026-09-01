# Guides

## Create a database

See [Get started](/en/d1/get-started/). `POST /v1/accounts/{accountId}/d1/databases` with `{ "name": "..." }`. Cloudflare REST is not provided.

## prepare / bind / batch

```ts
const stmt = env.DB.prepare("SELECT * FROM Customers WHERE CompanyName = ?");
const one = await stmt.bind("Bs Beverages").all();
const batch = await env.DB.batch([
  stmt.bind("Bs Beverages"),
  stmt.bind("Around the Horn"),
]);
```

`bind` supports SQLite `?` / `?NNN`. `batch` executes and commits sequentially.

## Sessions and bookmarks

```ts
const session = env.DB.withSession("first-primary");
const result = await session.prepare("SELECT 1 AS ok").all();
const bookmark = result.meta.duration !== undefined
  ? session.getBookmark()
  : null;
if (bookmark) {
  const later = env.DB.withSession(bookmark);
  await later.prepare("SELECT 1 AS ok").all();
}
```

A bookmark is an opaque string. It only preserves local sequential visibility on this database.

## dump()

```ts
try {
  await env.DB.dump();
} catch (error) {
  // hosted non-alpha: rejected
}
```

`dump()` is not a backup API. Platform backups are `ocd backup`, unrelated to the D1 binding.

## meta

`rows_read` / `rows_written` are local SQLite execution counters. `served_by_region` / `served_by_colo` / `served_by_primary` are not geography or replica identity.
