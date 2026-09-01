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

`html_handling`：`auto-trailing-slash`（默认）、`force-trailing-slash`、`drop-trailing-slash`、`none`。`not_found_handling`：`none`（默认）、`404-page`、`single-page-application`。`run_worker_first` 可以是 boolean，或一组以 `/` / `!/` 开头的路径规则。Assets-only 项目省略 `main`，也不能声明执行环境或 Worker-first。

可选 `publish_source_maps`。binding 存在时，`env.<binding>.fetch()` 只取资源，不进入租户 Worker。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| HTML trailing slash、SPA、Worker-first、`_headers` / `_redirects` 路由概念 | 是，见 [Cloudflare Static Assets](https://developers.cloudflare.com/workers/static-assets/) | 对齐 |
| 对象存储 | 全球 CDN | 该节点上的不可变 S3 对象，不是全球 CDN |
| 全球 CDN placement / 复制 / purge 传播 / 产品配额 | 是 | 不提供 |
| Pages 产品迁移向导 | 是 | 不提供 |

