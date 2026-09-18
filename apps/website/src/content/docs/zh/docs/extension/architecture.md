---
title: "调用如何发生"
description: "用户 Worker、facade、ocd、workerd 与 native Provider 如何通信；ocd 不代理业务字节。"
---

一次扩展调用有四个参与者和两条传输。`ocd` 校验 session 并交出 socket。之后 workerd 与 Provider 直接通信。

## 参与者

| 参与者      | 角色                                                                                                                   |
| ----------- | ---------------------------------------------------------------------------------------------------------------------- |
| 用户 Worker | 声明 `services` + `props`。调用 facade RPC，例如 `env.FILES.list()`。看不到 `HOST`。                                   |
| Facade      | 来自 `extension.toml` `[worker].main` 的 operator JavaScript。读取 `this.ctx.props`。调用 `env.HOST`。                 |
| workerd     | 唯一受监督的 pinned runtime。持有私有 `HostExtensionFactory`、broker fd 4 和 session Cap'n Proto client。              |
| `ocd`       | 启动时加载扩展、签发 session identity、经纪一对 socketpair、等待 Provider ACK，然后离开数据路径。                      |
| Provider    | operator 的原生进程。在标准输入（fd 0）上接受 `OCP1` + `SCM_RIGHTS`，ACK，并在 session socket 上提供 `HostExtension`。 |

仍然只有一个 workerd。Provider 是 `ocd` 的宿主子进程，不是第二个 runtime，也不是 tenant isolate。

## 调用顺序

```text
user Worker  -- Service Binding RPC + ctx.props -->  facade (workerd isolate)
facade       -- HOST.call / HOST.stream ---------->  session Cap'n Proto
                                                     (after the socket exists)

workerd  -- OCH1 + session identity / SCM_RIGHTS -->  ocd
ocd      -- OCP1 / SCM_RIGHTS + wait for ACK ------>  Provider
ocd      -- session FD back to workerd ------------>  workerd
workerd  <--------- session Cap'n Proto ----------->  Provider
```

1. 用户 Worker 调用 Service Binding。准入把 `services[].service` 解析为 tagged target：Worker ID 或扩展 slug。`props` 留在 Binding 上。
2. 受信任的 loader 实例化 facade，用 `HostExtensionFactory.get(sessionIdentity)` 注入 `HOST`，并把 `props` 作为 `ctx.props` 传入。`globalOutbound` 为 `null`。
3. 第一次使用 `HOST` 时，workerd 在 generation broker（继承的 fd 4）上发送 `OCH1`、session identity 和一个文件描述符。
4. `ocd` 在私有 registry 中查找该 identity（调用方 Worker、version 和 Binding；最多 1,024 个 session）。未知 identity fail closed。
5. 如有需要，`ocd` 启动 Provider，创建 socketpair，把 `OCP1` 和 pair 的一端发给 Provider control socket，并等待 ACK `0`。
6. ACK 之后，`ocd` 再次确认 session 仍然授权（避免 generation 或关闭竞争把晚到的 FD 交给失效调用），再把另一端交回 workerd。
7. workerd 与 Provider 在该 socket 上使用 Cap'n Proto `HostExtension`。unary `call` 与 streamed `openStream` 携带 operator 定义的方法编号和字节。

`ocd` 不代理、不解析、也不记录业务 payload。

## 为什么有两个 magic

| Magic  | Socket                            | 方向             | 用途                                        |
| ------ | --------------------------------- | ---------------- | ------------------------------------------- |
| `OCH1` | workerd generation broker（fd 4） | workerd → `ocd`  | 证明已校验的 session identity 并接收数据 FD |
| `OCP1` | Provider control（stdin，fd 0）   | `ocd` → Provider | 附加一个 session FD；Provider ACK `0`       |

两次传递都使用恰好一个 FD 的 `SCM_RIGHTS`。额外 FD、错误 magic、截断的 identity 或缺失 ACK 都会 fail closed。正式 FD passing 使用 workerd `--//:io_backend=cxx`。

## Session authority

registry 只为已验证的调用方 Worker、version 和 Binding 签发 session identity。不同 `props` 使用不同的 facade cache key 和 session。同一个扩展名字的多个 session 可以共享一个 Provider 进程。

Broker EOF 或 workerd generation 更换会关闭该 generation 的 session。Provider 可供下一个 generation 复用。`ocd` shutdown 以有界 TERM/KILL 回收 Provider。Provider 崩溃会使进行中的调用失败；后续 acquire 使用 200 ms–5 s backoff，连续六次失败后在本次 `ocd` 生命周期内保持 unavailable。

## 各层不该看到什么

| 秘密或句柄                   | 用户 Worker     | Facade | Provider                  | `ocd` 日志 |
| ---------------------------- | --------------- | ------ | ------------------------- | ---------- |
| `HOST` / `HostExtensionPort` | 否              | 是     | 不适用                    | 否         |
| Session identity             | 否              | 否     | 否                        | 否         |
| Provider 路径 / 原始 FD      | 否              | 否     | 自己的 stdin / session FD | 否         |
| 平台 token、S3、SQLite       | 否              | 否     | 否                        | 已脱敏     |
| `ctx.props`                  | 仅 Binding 配置 | 是     | 仅当 facade 放进 payload  | 否         |

## Cloudflare 边界

`services[].props` → `ctx.props` 与 Service Binding RPC 遵循 Cloudflare [Context](https://developers.cloudflare.com/workers/runtime-apis/context/) 和 [Service Binding RPC](https://developers.cloudflare.com/workers/runtime-apis/bindings/service-bindings/rpc/)。本地扩展解析、Provider 生命周期、`OCH1`/`OCP1` 和 `HOST` 只属于 open-compute。不要把它们当成托管 Cloudflare 能力，也不要计入 Worker API inventory。

限制、名字共享和 V7 干净 data directory 中断见[扩展 API](/zh/docs/extension/api/)。可跟随的例子见[实现一个扩展](/zh/docs/extension/tutorial/)。
