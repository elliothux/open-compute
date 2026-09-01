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

管理令牌从 `OPEN_COMPUTE_ADMIN_TOKEN` 读取，或由 `--token-env` 指定另一个变量名。不可将密钥值写入项目配置或命令参数。secret 对象只允许 `env` 键。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `env.TOKEN` 为 `string`；类型检查使用 `string` 而不是字面量 | 是，见 [Secrets](https://developers.cloudflare.com/workers/configuration/secrets/) | 是 |
| `wrangler secret put` | 是 | 不提供 |
| Cloudflare Secrets Store / 控制台密文 | 是 | 不提供 |
| 项目 JSON 中的 `file:` 引用 | 不适用 | 不允许（该形态属于 ocd 运维配置） |
| 缺失的环境变量 | 视命令而定 | `run` / `deploy` 失败 |

