# 开始

使用仓库示例和项目内精确锁定的 Wrangler。先启动 `ocd`，并确认 `GET /health/ready` 返回 200。

```sh
bun install --frozen-lockfile
./target/debug/ocd wrangler --project examples/hello-worker deploy --env dev
```

`ocd wrangler` 选择 local instance 或显式 remote target，核对 capability 公布的精确 pin，然后以 Wrangler 替换自身。认证、multipart upload、Versions、Deployments、Secrets、Static Assets 和资源 provisioning 都走 Cloudflare v4 合同。

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

下一步：[Wrangler 项目与部署目标](/zh/workers/projects)、[Workers 配置](/zh/workers/configuration/)和 [ocd 运维](/zh/ocd/)。
