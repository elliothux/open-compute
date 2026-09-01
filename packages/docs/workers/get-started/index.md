# 快速开始

使用 CLI 将模块 Worker 编译、校验并激活到已运行的本节点 `ocd`。`oc run` 不会再启动一个 workerd。平台尚未启动时，先完成 [ocd 上手](/ocd/get-started)。

```sh
bun run oc run --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd"
```

默认平台 origin 为 `http://127.0.0.1:8787`。该命令执行 TypeScript 检查、Rolldown 打包、创建或复用同名 Worker、校验并 promote，然后打印可访问 URL。源码变更后再次执行同一命令即可更新；当前不提供 watch / HMR。远端部署使用 `oc deploy`，且只接受 HTTPS origin。

## Hello Worker

仓库路径为 `examples/hello-worker/`。`open-compute.json`：

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

`src/index.ts`：

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

同目录还包含 `package.json`（`@open-compute/workers-types`）、`tsconfig.json`，以及由 `bun run oc types` 生成的 `worker-configuration.d.ts`。离线产物：

```sh
bun run oc build --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/open-compute.json
```

`build` 的 `--out` 必须指向尚不存在的文件。`types` 不需要 `ocd`。管理令牌读取 `OPEN_COMPUTE_ADMIN_TOKEN`（或 `--token-env`）。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 模块 Worker（`export default { fetch }`） | 是 | 是 |
| handler 签名、`Response.json`、`env` 注入 | 是，见 [Workers CLI 上手](https://developers.cloudflare.com/workers/get-started/guide/) | 是 |
| 类型包 | `@cloudflare/workers-types` | pinned `@open-compute/workers-types` |
| CLI | Wrangler | `oc`（`bun run oc ...`） |
| 项目文件 | wrangler.jsonc | `open-compute.json`（未知字段拒绝） |
| C3 脚手架 / Cloudflare 登录 / workers.dev 预览 / 控制台 | 是 | 不提供 |
| `--ocd` / `OPEN_COMPUTE_OCD` | 不适用 | 必须指向匹配版本的 `ocd`，用于编码 bundle；`run` 要求平台已在监听 |

下一步：[概念](/workers/concepts/)、[配置](/workers/configuration/)。
