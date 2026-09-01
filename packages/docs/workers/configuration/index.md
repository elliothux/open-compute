# 配置

Worker 项目文件是 `open-compute.json`。必须包含 `name`，并选择一种内容形态：`main`、`assets`、`main` + `assets`，或 `frameworkOutput`。parser 只接受下列已实现字段；未知字段会被拒绝。

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": { "GREETING": "Hello from TypeScript" }
}
```

## 字段

| 字段 | 作用 |
| --- | --- |
| `name` | Worker 名，`[a-z0-9]` 加内部连字符 |
| `main` | 入口 TS，相对配置文件目录 |
| `frameworkOutput` | 已构建框架产物；不能再和显式 `main` / `assets` 组合 |
| `tsconfig` | 默认 `tsconfig.json` |
| `vars` | 公开变量，JSON 值进入 `env` |
| `secrets` | `{ "TOKEN": { "env": "MY_TOKEN" } }`，只引用环境变量 |
| `bindings` | 对象：键是 `env` 名，值是 `{type, id, permissions?}`；DO / Workflow 还要 `className`；Workflow 可选 `schedules` |
| `services` | 数组 `[{binding, service, entrypoint?}]` |
| `assets` | `directory`、`binding?`、`run_worker_first`、`html_handling`、`not_found_handling`、`publish_source_maps` |
| `cache` | `enabled`、`cross_version_cache` |
| `exports` | 具名 Worker entrypoint 的 cache 覆盖，只接受 `{"type":"worker","cache":{...}}` |
| `images` | `{ "binding": "IMAGES" }` |
| `version_metadata` | `{ "binding": "VERSION", "tag"? }` |
| `accountId` | 覆盖默认账户 |
| `endpoint` | 平台 origin，默认 `http://127.0.0.1:8787` |

`main`、`frameworkOutput`、assets directory 与 `tsconfig` 相对配置文件目录解析，且不能逃逸项目边界。Assets-only 项目不能声明 vars、secrets、产品/service bindings，也不能要求 Worker-first。所有 binding 名共用同一 `env` 命名空间。文件最大 64 KiB，必须是正规 JSON（不是 jsonc）。

`bindings.type`：`kv_namespace`、`r2_bucket`、`d1_database`、`do_namespace`、`queue_producer`、`workflow`。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `vars`、secrets、assets routing、cache enabled、service bindings 语义 | 是，对照 [Wrangler configuration](https://developers.cloudflare.com/workers/wrangler/configuration/) | 字段名借用常见 Wrangler 配置；不是完整 `wrangler.jsonc` 兼容层 |
| 模块 Worker 的 `main` 指向 TS/JS 入口 | 是 | 是 |
| `compatibility_date` / `compatibility_flags` / `compatibilityDate` / `compatibilityFlags` | 是 | 不允许 |
| `workers_dev`、Custom Domains、`routes`、placement、observability、AI、vectorize | 是 | 不提供；未知键失败，不会忽略 |
| 控制面 | Cloudflare 控制面 | 本节点 `ocd` HTTP API |

## 本节

- [bindings](/workers/configuration/bindings)
- [compatibility dates](/workers/configuration/compatibility-dates)
- [compatibility flags](/workers/configuration/compatibility-flags)
- [Cron](/workers/configuration/cron-triggers)
- [vars](/workers/configuration/environment-variables)
- [secrets](/workers/configuration/secrets)
- [routing](/workers/configuration/routing)
