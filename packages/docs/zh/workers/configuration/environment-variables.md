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

`bun run oc types` 会把字面量写入 `worker-configuration.d.ts`（例如 `GREETING: "Hello from TypeScript"`）。更改 vars 后重新生成类型。离线 bundle 不包含 vars；`run` / `deploy` 才注入。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 公开字符串/JSON 注入 `env`，不是密钥 | 是，见 [Environment variables](https://developers.cloudflare.com/workers/configuration/environment-variables/) | 是 |
| Wrangler `[vars]` TOML | 是 | 不提供 |
| 控制台编辑 / Wrangler environments 产品 | 是 | 不提供 |
| 未知顶层键 | 可能被忽略 | 整个项目配置失败 |
| 密钥存放位置 | Secrets 产品 | [secrets](/zh/workers/configuration/secrets)，不可放入 `vars` |

