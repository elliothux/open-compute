# 环境变量

`vars` 是公开、非密钥的值，以 structured-clone 兼容的 JSON 进入 `env`。

```json
{
  "name": "hello-typescript",
  "main": "src/index.ts",
  "vars": {
    "GREETING": "Hello from TypeScript"
  }
}
```

```ts
export default {
  fetch(_request: Request, env: Env): Response {
    return new Response(env.GREETING);
  },
} satisfies ExportedHandler<Env>;
```

`bun run oc types` 会把字面量写进 `worker-configuration.d.ts`（例如 `GREETING: "Hello from TypeScript"`）。改 vars 后重新生成类型。离线 bundle 不包含 vars；`run` / `deploy` 才注入。

## 与 Cloudflare 相同

公开字符串/JSON 挂到 `env`，不是密钥。对照 [Environment variables](https://developers.cloudflare.com/workers/configuration/environment-variables/)。

## 故意不同

没有 Wrangler `[vars]` TOML、没有 dashboard 编辑、没有 per-environment wrangler environments 产品。未知顶层键仍会让整个项目配置失败。密钥走 [secrets](/workers/configuration/secrets)，不要放进 `vars`。
