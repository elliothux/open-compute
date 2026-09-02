# 上手

通过精确锁定的 Wrangler client 部署模块 Worker：

```sh
CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4 \
CLOUDFLARE_API_TOKEN=<token> \
CLOUDFLARE_ACCOUNT_ID=<account-id> \
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

示例是标准 Wrangler 项目，包含 `name`、`main`、`compatibility_date`、`workers_dev: false` 和 `vars`。在线 upload 与 activation 由 `wrangler@4.127.1` 负责。

本地命令仍然保留：

```sh
bun run oc build --config examples/hello-worker/wrangler.jsonc \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/wrangler.jsonc
```

`build` 使用 TypeScript 7 做类型检查、使用 Rolldown bundle、校验配置的 assets，并写出单一 Worker bundle；不会覆盖已有文件。assets-only 项目直接用 Wrangler 部署。`types` 默认写入 `worker-configuration.d.ts`。

下一步：[配置](/zh/workers/configuration/)和[版本与部署](/zh/workers/versions-and-deployments/)。
