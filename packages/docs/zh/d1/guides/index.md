# 指南

## 创建 database

见[上手](/zh/d1/get-started/)。database CRUD 与 query 通过官方 SDK 或固定 Wrangler 使用 `/client/v4/accounts/{account_id}/d1/database`。

## prepare / bind / batch

```ts
const stmt = env.DB.prepare("SELECT * FROM Customers WHERE CompanyName = ?");
const one = await stmt.bind("Bs Beverages").all();
const batch = await env.DB.batch([
  stmt.bind("Bs Beverages"),
  stmt.bind("Around the Horn"),
]);
```

`bind` 支持 SQLite 的 `?` / `?NNN`。`batch` 顺序执行并提交。

## session 与 bookmark

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

bookmark 是不透明字符串，只保证本库本地顺序可见性。

## dump()

```ts
try {
  await env.DB.dump();
} catch (error) {
  // hosted 非 alpha：拒绝
}
```

`dump()` 不是备份 API。平台备份走 `ocd backup`，与 D1 binding 无关。

## meta

`rows_read` / `rows_written` 是本地 SQLite 执行计数。`served_by_region` / `served_by_colo` / `served_by_primary` 不是地理或 replica 身份。
