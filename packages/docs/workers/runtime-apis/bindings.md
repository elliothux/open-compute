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

## 与 Cloudflare 相同

`env.BINDING` 的类型与 [Bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/) 和 [Service bindings](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/) 相同。Version Metadata 字段见 [version metadata](https://developers.cloudflare.com/workers/runtime-apis/bindings/version-metadata/)。KV / R2 / D1 / DO / Queue / Workflow / Assets / Images 的成员签名去各产品文档，不要从本页抄。

## 故意不同：OC-SERVICE-001

Service Bindings 在一个平台权威内提供默认/具名 fetch 和 RPC。它们不声称 Cloudflare 跨地域 placement 或全球 service discovery；目标准入、deployment pin、capability 生命周期和恢复都是本地的，失败则 fail closed。

没有 Workers for Platforms dispatcher、没有 Dynamic Worker Loaders 作为租户产品、没有 mTLS / Rate Limit / Secrets Store / AI binding。
