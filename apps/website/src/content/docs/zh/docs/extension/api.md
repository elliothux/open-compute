---
title: "扩展 API"
description: "Operator 配置、extension.toml、Wrangler services 与 props，以及仅 facade 可见的 HOST 端口。"
---

本页是本地原生扩展的 operator 与 Worker 合同。它不是 Cloudflare 托管 Workers API。

## `[extensions.<name>]`

```toml
[extensions.local-files]
path = "./extensions/files"
```

| 字段          | 含义                                                                                                                  |
| ------------- | --------------------------------------------------------------------------------------------------------------------- |
| 表键 `<name>` | 用户 Worker 写在 `services[].service` 里的服务名。lowercase ASCII slug，1–63 字符，首尾为字母或数字，中间可含连字符。 |
| `path`        | 包含 `extension.toml` 的目录。相对路径相对实际加载的 config file 所在目录解析，解析后必须是绝对路径。                 |

未知字段会被拒绝。名字与 live Worker service 共用 namespace。启动会拒绝与已有 Worker 冲突的扩展；之后创建 Worker 或上传 Service 也会拒绝与扩展冲突的 Worker。

仅 macOS 与 Linux。本版本没有 Windows Provider 路径。

## `extension.toml`

```toml
[worker]
main = "worker/index.js"

[native]
executable = "native/files-provider"
```

| 段         | 字段         | 含义                         |
| ---------- | ------------ | ---------------------------- |
| `[worker]` | `main`       | facade 模块的相对路径，UTF-8 |
| `[native]` | `executable` | Provider 二进制的相对路径    |

路径必须是只含普通分量的相对路径（不能是 `.`、`..`、绝对路径或符号链接）。`ocd` 用 `NOFOLLOW` 打开目录、manifest、facade 和 executable。限制：manifest 64 KiB，facade 64 MiB，executable 256 MiB。executable 必须是普通文件、具备 owner 执行位，且不能对 group/other 可写。

没有 cwd fallback、网络下载、动态库加载、version 字段或热更新。错误会使启动失败。

## Wrangler `services` + `props`

用户 Worker 继续使用标准 Service Binding 字段。不要发明 Wrangler `extensions` 数组或新的 `type`。

```json
{
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "entrypoint": "default",
      "props": { "directory": "invoices" }
    }
  ]
}
```

| 字段         | 含义                                         |
| ------------ | -------------------------------------------- |
| `binding`    | 用户 Worker `env` 上的名字                   |
| `service`    | 共享 namespace 中的扩展 slug 或 Worker 名    |
| `entrypoint` | 可选的 facade entrypoint                     |
| `props`      | 不可变 JSON；facade 以 `this.ctx.props` 读取 |

`props` 是 Binding 参数，不是 Provider 配置。它们不会进入 Provider argv、environment 或可变全局状态。facade 把 Provider 需要的值复制进 `HOST` payload。

部署保存 schema 2 的 tagged Service descriptor：普通 Worker 保存 stable Worker ID，本地扩展保存 lowercase slug。部署固定名字、entrypoint 和 canonical props，不固定扩展文件字节。

schema V7 是明确的 Day1 break：若 data directory 仍有旧 `version_services` 行，migration 会原子失败并要求干净的 data directory。没有 backfill。

## Facade `HOST`

只有配置中的 facade 收到 `env.HOST`。TypeScript 形状为：

```ts
interface HostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}
```

| 成员                      | 用途                       |
| ------------------------- | -------------------------- |
| `call(method, payload)`   | unary 请求与响应           |
| `stream(method, payload)` | unary 请求，流式响应 chunk |

`method` 是 operator 选择的 `UInt32`。payload 是不透明字节。unary 请求和响应各不超过 8 MiB。stream chunk 不超过 64 KiB，总读取不超过 64 MiB。既有 Service authority 提供 30 秒 root deadline。

session socket 上的 Cap'n Proto schema 是 pinned workerd 中的 `HostExtension` / `HostExtensionStream`（`call` 与 `openStream`）。`HostExtensionFactory.get(sessionIdentity)` 只绑定到受信任的 loader 和 Durable Object host system Worker。该 port 只能通过 dynamic env capability table 委派一次，不能持久化或再次 RPC 转移。

## Provider 进程

| 主题     | 合同                                                                                                                 |
| -------- | -------------------------------------------------------------------------------------------------------------------- |
| 启动     | 该扩展名字的第一次 session；每个名字一个 process group                                                               |
| 身份     | executable 字节与 FD 在 `ocd` 启动时固定                                                                             |
| 环境     | 清空；argv 为空；control socket 为标准输入（fd 0）                                                                   |
| 工作目录 | `<data.path>/runtime/extensions/<name>`                                                                              |
| Attach   | magic `OCP1`、恰好一个 `SCM_RIGHTS` FD、ACK 字节 `0`                                                                 |
| 崩溃     | 进行中的调用失败；后续 acquire 按 200 ms–5 s backoff 重试；连续六次失败后，本次 `ocd` 生命周期保持 unavailable       |
| 关闭     | Broker EOF 或 workerd generation 更换会关闭 session；Provider 可供新 generation 复用，并在 `ocd` shutdown 时有界回收 |

最多 1,024 个 live session。每个 Binding（名字 + props + 调用方）使用自己的 session identity。

## Fail closed

`ocd` 不安装、下载、版本管理、授权、热更新、sandbox 或 HTTP-fallback 扩展。普通 Worker 拿不到 `HOST`。被移除的扩展是 unavailable，不会解析成同名 Worker。ABI 不匹配、额外 FD、未知 identity、timeout 和 disconnect 都是错误。

参见[实现一个扩展](/zh/docs/extension/tutorial/)和[调用如何发生](/zh/docs/extension/architecture/)。
