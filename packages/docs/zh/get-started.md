# 开始

将仓库中的 hello Worker 部署到已就绪的 `ocd`。

## 1. 安装 ocd，确认 `/health/ready`

按[安装与首次启动](/zh/ocd/get-started)运行 `ocd`。本页假定 `GET /health/ready` 已返回 200。`oc run` 不会再启动 workerd；它将 Worker 部署到已在运行的本地平台。

## 2. Hello Worker

使用仓库中的 `examples/hello-worker`。项目文件为 `open-compute.json`：

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

入口为模块 Worker：

```ts
export default {
  fetch(request: Request, env: Env): Response {
    return Response.json({
      message: env.GREETING,
      pathname: new URL(request.url).pathname,
    });
  },
} satisfies ExportedHandler<Env>;
```

## 3. 获取 Worker URL

在仓库根目录执行：

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd <path-to-ocd>
```

成功时打印 Worker URL。将 `<path-to-ocd>` 替换为 `ocd` 二进制路径，或设置环境变量 `OPEN_COMPUTE_OCD`。

## `open-compute.json` 不是 `wrangler.jsonc`

配置借用了 Wrangler 的部分字段名（`name`、`main`、`vars`），但 **`open-compute.json` 不是完整的 `wrangler.jsonc`**。未知字段将被拒绝。项目 JSON 不含 `compatibilityDate`：兼容日期由平台锁定，见 `ocd capabilities --json` 中的 `runtime.effective_compatibility_date`。

下一步：[产品目录](/zh/directory)。将 `ocd` 作为服务运行时，见[运维](/zh/ocd/)。
