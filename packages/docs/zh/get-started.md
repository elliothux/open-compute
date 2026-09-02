# 开始

使用仓库示例和精确锁定的 Wrangler transport。先启动 `ocd`，并确认 `GET /health/ready` 返回 200。

```sh
export CLOUDFLARE_API_BASE_URL=http://127.0.0.1:8787/client/v4
export CLOUDFLARE_API_TOKEN=<token>
export CLOUDFLARE_ACCOUNT_ID=<account-id>
bun run oc deploy --config examples/hello-worker/wrangler.jsonc
```

`oc deploy` 只是 `wrangler@4.127.1 deploy` 的薄封装。认证、multipart upload、Versions、Deployments、Secrets、Static Assets 和资源 provisioning 都走 Cloudflare v4 合同。

项目使用标准 Wrangler 配置：

```json
{
  "$schema": "../../node_modules/wrangler/config-schema.json",
  "name": "hello-typescript",
  "main": "src/index.ts",
  "compatibility_date": "2026-08-30",
  "workers_dev": false,
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

离线校验使用 `oc build`：保留仓库的 TypeScript 7 与 Rolldown 检查，并输出单一 Worker bundle。`oc types` 生成本地 `Env` 类型；两者都不访问管理 API。Static Assets 只在本地校验，由 Wrangler 上传。

下一步：[Workers 配置](/zh/workers/configuration/)和 [ocd 运维](/zh/ocd/)。
