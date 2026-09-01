# 开始

最短路径：`ocd` 已就绪，然后跑仓库里的 hello Worker。

## 1. ocd 已安装，且 `/health/ready`

先按[安装与首次启动](/ocd/get-started)把 `ocd` 跑起来。本页假设 `GET /health/ready` 已经返回 200。`oc run` 不会再起一个 workerd，它把 Worker 激活到已经在跑的本地平台。

## 2. Hello Worker

用仓库里的 `examples/hello-worker`。项目文件是 `open-compute.json`：

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

入口是模块 Worker：

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

## 3. 打印一个 URL

在仓库根目录：

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd <path-to-ocd>
```

成功时打印 Worker URL。把 `<path-to-ocd>` 换成这台机器上的 `ocd` 二进制（或设 `OPEN_COMPUTE_OCD`）。

## `open-compute.json` 不是 `wrangler.jsonc`

配置借用了 Wrangler 的部分字段名（`name`、`main`、`vars`），但 **`open-compute.json` 不是 `wrangler.jsonc`**。未知字段直接拒绝。项目 JSON 没有 `compatibilityDate`：compatibility date 由平台冻结，见 `ocd capabilities --json` 的 `runtime.effective_compatibility_date`。

下一步：[产品目录](/directory)。还要把 `ocd` 当服务跑时，去[运维](/ocd/)。
