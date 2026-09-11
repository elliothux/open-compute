---
title: "Known issues"
---

本页记录当前二进制上会遇到的限制，不是路线图。

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
```

源码变更后必须再次执行该命令。

## 兼容性

| 主题                                | Cloudflare                | open-compute                                                             |
| ----------------------------------- | ------------------------- | ------------------------------------------------------------------------ |
| 模块 isolate                        | 是                        | 是                                                                       |
| 本地 watch / HMR（`wrangler dev`）  | 是                        | 提供；这是不访问 `ocd` 的上游本地开发流程                                |
| 远程开发（`wrangler dev --remote`） | 是                        | 不支持；使用显式 dev/staging Deployment                                  |
| 项目文件                            | `wrangler.jsonc`          | 使用相同的固定 Wrangler JSONC schema；不支持的 server 能力 fail closed   |
| 控制面                              | Cloudflare 控制面         | `/client/v4` 下的受支持 Cloudflare v4 子集与已文档化 extension operation |
| workers.dev                         | 是                        | 不提供                                                                   |
| Vite plugin 作为本产品开发服务器    | `@cloudflare/vite-plugin` | framework adapter 通过 `.wrangler/deploy/config.json` handoff            |
| 出站网络策略                        | Cloudflare 托管 TCP 策略  | 见 [TCP sockets](/docs/zh/workers/runtime-apis/tcp-sockets)              |
| 请求级 CPU / subrequest 配额        | 是                        | 不执行；见 [限制](/docs/zh/workers/platform/limits)                      |

对照 Cloudflare 的 [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) 没有意义：那是托管 fleet 的清单。
