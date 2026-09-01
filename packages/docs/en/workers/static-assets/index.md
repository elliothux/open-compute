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

`html_handling`: `auto-trailing-slash` (default), `force-trailing-slash`, `drop-trailing-slash`, `none`. `not_found_handling`: `none` (default), `404-page`, `single-page-application`. `run_worker_first` may be a boolean or a list of path rules starting with `/` or `!/`. Assets-only projects omit `main` and cannot declare an execution environment or Worker-first.

Optional `publish_source_maps`. When a binding is present, `env.<binding>.fetch()` serves assets only and never enters the tenant Worker.

## Compatibility

| Topic | Cloudflare | open-compute |
| --- | --- | --- |
| HTML trailing slash, SPA, Worker-first, `_headers` / `_redirects` routing concepts | Yes — [Cloudflare Static Assets](https://developers.cloudflare.com/workers/static-assets/) | Aligned |
| Object storage | Global CDN | Immutable S3 objects on this node, not a global CDN |
| Global CDN placement / replication / purge propagation / product quotas | Yes | Not provided |
| Pages migration wizard | Yes | Not provided |

