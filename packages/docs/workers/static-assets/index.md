# Static Assets

把静态文件冻进同一份不可变 deployment。配置在 `assets`。

```json
{
  "name": "site",
  "main": "src/index.ts",
  "assets": {
    "directory": "./public",
    "binding": "ASSETS",
    "run_worker_first": false,
    "html_handling": "auto-trailing-slash",
    "not_found_handling": "none"
  }
}
```

`html_handling`：`auto-trailing-slash`（默认）、`force-trailing-slash`、`drop-trailing-slash`、`none`。`not_found_handling`：`none`（默认）、`404-page`、`single-page-application`。`run_worker_first` 可以是 boolean，或一组以 `/` / `!/` 开头的路径规则。Assets-only 项目不要 `main`，也不能声明执行环境或 Worker-first。

可选 `publish_source_maps`。binding 存在时，`env.<binding>.fetch()` 只取资源，不进租户 Worker。

## 与 Cloudflare 相同

HTML trailing slash、SPA、Worker-first、`_headers` / `_redirects` 这些**路由概念**与 [Cloudflare Static Assets](https://developers.cloudflare.com/workers/static-assets/) 对齐。不要在本页重写 Cloudflare 的路由细则，去原文。

## 故意不同：OC-ASSETS-001

Static Assets 是不可变的、S3 后端的部署内容，由单节点平台提供。Routing 和 binding 行为有覆盖，但 Cloudflare 的全球 CDN placement、replication、purge 传播和产品配额不提供。

没有 Pages 产品迁移向导、没有全球 purge、没有套餐配额。对象字节在你配置的 S3-compatible store 上。
