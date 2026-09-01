# Known issues

本页记录当前二进制上会遇到的限制，不是路线图。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
```

源码变更后必须再次执行该命令。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 模块 isolate | 是 | 是 |
| watch / HMR（`wrangler dev`） | 是 | 不提供；`oc run` 不启动 workerd，也不监视文件 |
| 项目文件 | wrangler.jsonc（可含注释） | `open-compute.json`；未知字段将被拒绝；无 `compatibilityDate`、无完整 Wrangler 产品键、无 jsonc 注释 |
| 控制面 | Cloudflare 控制面 | 本机 `ocd` HTTP API（`/v1/account`、`/v1/accounts/.../workers`） |
| workers.dev | 是 | 不提供 |
| Vite plugin 作为本产品开发服务器 | `@cloudflare/vite-plugin` | 不提供；框架产物通过 `frameworkOutput` 导入 |
| 出站网络策略 | Cloudflare 托管 TCP 策略 | 见 [TCP sockets](/workers/runtime-apis/tcp-sockets) |
| 请求级 CPU / subrequest 配额 | 是 | 不执行；见 [限制](/workers/platform/limits) |

对照 Cloudflare 的 [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) 没有意义：那是托管 fleet 的清单。

