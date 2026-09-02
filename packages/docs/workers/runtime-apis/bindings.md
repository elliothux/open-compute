# Bindings (`env`)

`env` 只包含部署声明过的名字。Version Metadata 是平台注入的只读对象：`id`、`tag`、`timestamp`。

```ts
export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const version = env.VERSION.id;
    return env.AUTH.fetch(request);
  },
} satisfies ExportedHandler<Env>;
```

```json
{
  "name": "front",
  "main": "src/index.ts",
  "services": [{ "binding": "AUTH", "service": "auth-worker" }],
  "version_metadata": { "binding": "VERSION", "tag": "release-1" }
}
```

Service Binding：默认/具名 `fetch` 和 RPC。目标必须是同账户、可解析的唯一 Worker 名；部署时冻结为目标 ID。可选 `entrypoint`。

KV / R2 / D1 / DO / Queue / Workflow / Assets / Images 的成员签名见各产品文档。配置语法见 [绑定](/workers/configuration/bindings)。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| `env.BINDING` 类型 | 是，见 [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) 与 [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) | 是 |
| Version Metadata 字段 | 是，见 [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/) | `id`、`tag`、`timestamp` |
| Service Bindings | 跨地域 placement / 全球 service discovery | 仅限本平台；默认/具名 fetch 与 RPC；调用方准入与部署钉扎均在本机判定；失败则关闭 |
| Workers for Platforms dispatcher / Dynamic Worker Loaders | 是 | 不提供 |
| mTLS / Rate Limit / Secrets Store / AI binding | 是 | 不提供 |

