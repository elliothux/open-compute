# Known issues

诚实清单，不是路线图。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
```

改源码后必须再跑这一条；没有 `wrangler dev` 热更新。

## 与 Cloudflare 相同

Worker 仍是模块 isolate。已知问题页对应对手的 [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) 导航位置，不是同一份清单。

## 故意不同

- **没有 watch / HMR。** 改源码后重新执行 `bun run oc run ...`。`oc run` 不启动 workerd，也不监视文件。
- **`open-compute.json` ≠ `wrangler.jsonc`。** 未知字段拒绝。没有 `compatibilityDate`、没有完整 Wrangler 产品键、没有 jsonc 注释。
- **没有 Cloudflare REST v4。** 控制面是本机 `ocd` HTTP API（`/v1/account`、`/v1/accounts/.../workers`）。不要拿 Wrangler / Cloudflare API token 去打这个平台。
- **没有 playground、没有 dashboard 编辑器、没有 `workers.dev`。**
- **没有 Vite plugin 作为本产品的开发服务器。** 框架产物走 `frameworkOutput` 导入，不是 `@cloudflare/vite-plugin` 的 wrangler 工作流。
- **出网政策不是托管 Cloudflare TCP 策略。** 见 [`OC-WKR-TCP-001`](/workers/runtime-apis/tcp-sockets)。
- **request-scoped CPU / subrequest quota 未执行。** 见 [`OC-WKR-LIMIT-001`](/workers/platform/limits)。

对照 Cloudflare 的 [known issues](https://developers.cloudflare.com/workers/platform/known-issues/) 没有意义：那是他们的托管 fleet。本页只记录这个二进制上会碰到的差别。
