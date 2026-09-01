# Secrets

密钥只能引用环境变量：`"secrets": { "TOKEN": { "env": "MY_TOKEN" } }`。只有 `run` / `deploy` 才读取其值。离线 bundle 不含密钥。

```json
{
  "name": "secure",
  "main": "src/index.ts",
  "secrets": {
    "TOKEN": { "env": "MY_TOKEN" }
  }
}
```

```ts
export default {
  fetch(_request: Request, env: Env): Response {
    return new Response(env.TOKEN ? "present" : "missing");
  },
} satisfies ExportedHandler<Env>;
```

管理令牌从 `OPEN_COMPUTE_ADMIN_TOKEN` 读取，或 `--token-env` 指定另一个变量名。不要把密钥值写入项目配置或命令参数。secret 对象只允许 `env` 键。

## 与 Cloudflare 相同

`env.TOKEN` 是 string；类型检查里是 `string` 而不是字面量。对照 [Secrets](https://developers.cloudflare.com/workers/configuration/secrets/)。

## 故意不同

没有 `wrangler secret put`、没有 Cloudflare Secrets Store、没有 dashboard 密文。没有 `file:` 引用出现在 **项目** JSON 里（那是 ocd 运维配置的密钥引用形态）。缺失的环境变量会让 `run` / `deploy` 失败。
