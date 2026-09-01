# 上手

用 CLI 把一个模块 Worker 编译、校验并激活到已经在跑的本机 `ocd`。`oc run` 不会再起一个 workerd。平台还没起来时，先看 [ocd 上手](/ocd/get-started)。

```sh
bun run oc run --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd"
```

默认平台 origin 是 `http://127.0.0.1:8787`。命令会做 TS 检查、Rolldown 打包、创建或复用同名 Worker、校验并 promote，然后打印可访问 URL。改源码后再跑同一条命令即可更新；当前没有 watch / HMR。远端用 `oc deploy`，只接受 HTTPS origin。

## 与 Cloudflare 相同

还是 ES module Worker：`export default { fetch }`。handler 签名、`Response.json`、`env` 注入和 [Cloudflare Workers CLI 上手](https://developers.cloudflare.com/workers/get-started/guide/) 里的编程模型一致。类型来自 pinned `@open-compute/workers-types`（对应 pinned `@cloudflare/workers-types`）。

## 故意不同

CLI 是 `oc`（`bun run oc ...`），不是 Wrangler。项目文件是 `open-compute.json`，不是完整 `wrangler.jsonc`。没有 C3 脚手架、没有 Cloudflare 登录、没有 `workers.dev` 预览、没有 dashboard。`--ocd`（或 `OPEN_COMPUTE_OCD`）必须指向匹配版本的 `ocd`，用来编码 bundle；`run` 要求平台已经在听。

## Hello Worker 文件

仓库 `examples/hello-worker/`。`open-compute.json`：

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

同目录还有 `package.json`（`@open-compute/workers-types`）、`tsconfig.json` 和 `bun run oc types` 生成的 `worker-configuration.d.ts`。离线产物：

```sh
bun run oc build --config examples/hello-worker/open-compute.json \
  --ocd "$PWD/target/debug/ocd" --out /absolute/new-worker.bundle
bun run oc types --config examples/hello-worker/open-compute.json
```

`build` 的 `--out` 必须是还不存在的文件。`types` 不需要 `ocd`。管理令牌读 `OPEN_COMPUTE_ADMIN_TOKEN`（或 `--token-env`）。下一步：[概念](/workers/concepts/)、[配置](/workers/configuration/)。
