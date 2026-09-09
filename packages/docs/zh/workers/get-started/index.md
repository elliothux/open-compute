# 上手

安装 frozen workspace dependency、启动一个本机 `ocd`，再通过项目内精确固定版本的 Wrangler client 部署模块 Worker：

```sh
bun install --frozen-lockfile
cd examples/hello-worker
ocd wrangler deploy --env dev
```

示例是标准 Wrangler 项目，包含 `name`、`main`、`compatibility_date`、`workers_dev: false` 和 `vars`。在线 upload 与 activation 由 `wrangler@4.127.1` 负责。

本地命令仍然保留：

```sh
bun run oc build --config examples/hello-worker/wrangler.jsonc \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/wrangler.jsonc
```

`build` 使用 TypeScript 7 做类型检查、使用 Rolldown bundle、校验配置的 assets，并写出单一 Worker bundle；不会覆盖已有文件。assets-only 项目直接用 Wrangler 部署。`types` 默认写入 `worker-configuration.d.ts`。

下一步：[Wrangler 项目与部署目标](/zh/workers/projects)、[配置](/zh/workers/configuration/)和[版本与部署](/zh/workers/versions-and-deployments/)。
