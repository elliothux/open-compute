---
title: "实现一个扩展"
description: "动手实现 local-files 扩展：manifest、facade Worker、native Provider、ocd 配置，以及用户 Worker 绑定。"
---

本教程实现一个 `local-files` 扩展：列出并读取 operator 放在 Provider 工作目录旁的文件。用户 Worker 用 Wrangler `services` 和 `props` 绑定它。没有新的公开 Binding 类型。

需要一台正在运行的 macOS 或 Linux `ocd`、可写配置文件，以及能处理 Unix `SCM_RIGHTS` 与 Cap'n Proto 的原生工具链。open-compute 仓库自带一个完整可运行的 Provider：测试 fixture [crates/service/src/bin/host_extension_test_provider/](https://github.com/elliothux/open-compute/blob/main/crates/service/src/bin/host_extension_test_provider/main.rs)——schema、Cap'n Proto 绑定与 attach 循环共约 300 行 Rust。把它当作你自己的 Provider 的活参考；它是 `test-support` 二进制，不进入 release artifact。

## 1. 创建扩展目录

把目录放在将要引用它的配置文件附近。manifest 内的路径必须是相对路径，不能包含 `.` 或 `..`，打开时不跟随符号链接。

```text
extensions/files/
  extension.toml
  worker/index.js
  native/files-provider
```

`extension.toml` 只有两个必填段：

```toml
[worker]
main = "worker/index.js"

[native]
executable = "native/files-provider"
```

facade 必须是 UTF-8 JavaScript（或编译成 JS 的源），最大 64 MiB。executable 必须是普通文件、owner 可执行、且不能对 group/other 可写，最大 256 MiB。`extension.toml` 本身最大 64 KiB。未知字段会被拒绝。

## 2. 写 facade

facade 是 Service Binding 目标。它从 `this.ctx.props` 读取不可变 Binding 参数，并且是唯一收到 `env.HOST` 的 Worker：

```ts
interface HostExtensionPort {
  call(method: number, payload: Uint8Array): Promise<Uint8Array>;
  stream(method: number, payload: Uint8Array): ReadableStream<Uint8Array>;
}
```

一个 local-files facade：用 unary 方法 `1` 列出目录，用 stream 方法 `2` 读取文件：

```js
import { WorkerEntrypoint } from "cloudflare:workers";

const encode = (value) => new TextEncoder().encode(value);

export default class Files extends WorkerEntrypoint {
  async list() {
    return new TextDecoder().decode(
      await this.env.HOST.call(1, encode(this.ctx.props.directory)),
    );
  }

  async read(name) {
    return new Response(
      this.env.HOST.stream(2, encode(`${this.ctx.props.directory}/${name}`)),
    ).text();
  }
}
```

facade 的 `globalOutbound` 为 `null`。它不能访问公网、平台 listener，也看不到 S3、SQLite、loader key 或内部 token。业务 codec、路径校验和宿主权限仍由这份 operator 代码和 Provider 负责。

`HostExtensionFactory` 只存在于受信任的 loader。不要导入它、持久化 `HOST`，也不要把该 port 再次 RPC 转移。

## 3. 写 native Provider

`ocd` 在该扩展名字的第一次 session 时启动一个 Provider 进程：

- 使用启动时已打开并固定身份的 executable；
- 清空 environment，argv 为空；
- 工作目录是 `<data.path>/runtime/extensions/<name>`（不是源码目录）；
- control socket 作为标准输入（fd 0）继承。

control socket 只用于 session attach。业务字节不经过 `ocd`。

Provider 在标准输入上循环：

1. 读取 4 字节外加一个文件描述符（`SCM_RIGHTS`）。
2. 要求 magic 为 `OCP1`，且恰好一个 FD。
3. 把该 FD 当作 Cap'n Proto two-party session。
4. 写一个 ACK 字节 `0`。

session schema 是：

```text
interface HostExtension {
  call @0 (method :UInt32, payload :Data) -> (payload :Data);
  openStream @1 (method :UInt32, payload :Data) -> (stream :HostExtensionStream);
}

interface HostExtensionStream {
  read @0 (maxBytes :UInt32) -> (payload :Data, eof :Bool);
  cancel @1 ();
}
```

方法编号和 payload codec 由你定义。unary 请求和响应各不超过 8 MiB。stream 每个 chunk 不超过 64 KiB，总读取不超过 64 MiB。

本教程中，方法 `1` 列出 payload 给出的相对目录（UTF-8、无 `/` 前缀、无 `..`）中的普通文件，返回换行分隔的名单。方法 `2` 打开该相对路径并流式返回字节。用 `openat` 加 `O_NOFOLLOW` 从进程工作目录（或 Provider 自己打开的 operator 根）打开。`props` 不会进入 argv、environment 或可变全局配置；facade 必须把需要的值放进 payload。上面链接的 fixture 完整实现了这套契约，包括 `O_NOFOLLOW` 路径遍历。

ABI 不匹配、额外 FD、未知 identity、attach timeout 或 disconnect 都会 fail closed。取消调用不会回滚 Provider 副作用，也不会重放请求。

## 4. 在 `ocd` 配置里登记

```toml
[extensions.local-files]
path = "./extensions/files"
```

名字 `local-files` 是 lowercase ASCII slug（1–63 字符，首尾为字母或数字，中间可含连字符）。它与 Worker service 名字共用 namespace：启动、创建 Worker、上传 Service 都会拒绝冲突。

检查后重启。扩展不热更新。

```sh
ocd --config /etc/open-compute/config.toml config check
ocd restart
```

`config check` 只校验 TOML。真正打开 `extension.toml`、facade 和 executable 的是启动过程。文件不合格时 `ocd` 拒绝启动。

## 5. 从用户 Worker 绑定

在应用的 `wrangler.jsonc` 里声明 Service Binding，`service` 填扩展名：

```json
{
  "name": "billing",
  "main": "src/index.ts",
  "compatibility_date": "2026-09-08",
  "services": [
    {
      "binding": "FILES",
      "service": "local-files",
      "props": { "directory": "invoices" }
    }
  ]
}
```

同一个扩展、不同 `props` 的两个 Binding 使用不同的 facade cache key 和 session，可以共享同一个 Provider 进程。部署固定名字、entrypoint 和 canonical props；operator 替换文件并重启 `ocd` 后，现有部署使用新实现。

像调用其它 Service Binding RPC 一样使用它：

```ts
export default {
  async fetch(_request: Request, env: Env): Promise<Response> {
    const listing = await env.FILES.list();
    const body = await env.FILES.read("a.txt");
    return new Response(`${listing}\n${body}`);
  },
} satisfies ExportedHandler<Env>;
```

```sh
ocd wrangler deploy
```

用户 Worker 看不到 `HOST`，只看到 facade 导出的方法。

## 6. 确认边界

- 用户 Worker 通过 Service Binding RPC 调用 facade（`ctx.props` 语义遵循 Cloudflare [Context](https://developers.cloudflare.com/workers/runtime-apis/context/) 与 [Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)）。
- facade 通过 `HOST.call` / `HOST.stream` 调用 Provider。
- `ocd` 只经纪一个 session socket，然后离开数据路径。

握手细节见[调用如何发生](/zh/docs/extension/architecture/)。配置与 `HOST` 面见[扩展 API](/zh/docs/extension/api/)。
