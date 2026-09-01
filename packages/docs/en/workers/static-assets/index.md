# Static Assets

Static files freeze into the same immutable deployment. Configure them under `assets`.

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

`html_handling`: `auto-trailing-slash` (default), `force-trailing-slash`, `drop-trailing-slash`, `none`. `not_found_handling`: `none` (default), `404-page`, `single-page-application`. `run_worker_first` may be a boolean or a list of path rules starting with `/` or `!/`. Assets-only projects omit `main` and cannot declare an execution environment or Worker-first. Optional `publish_source_maps`. When a binding is present, `env.<binding>.fetch()` serves assets only and never enters the tenant Worker.

## Same as Cloudflare

HTML trailing slash, SPA, Worker-first, `_headers` / `_redirects` **routing concepts** match [Cloudflare Static Assets](https://developers.cloudflare.com/workers/static-assets/). Do not rewrite Cloudflare's routing details here; follow that page.

## Intentional delta: OC-ASSETS-001

Static Assets are immutable S3-backed deployment content served by the single-node platform. Routing and binding behavior are covered, but Cloudflare's global CDN placement, replication, purge propagation, and product quotas are not provided.

No Pages migration wizard, no global purge, no plan quotas. Object bytes live on the S3-compatible store you configured.
