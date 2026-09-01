# Versions and deployments

一次部署：创建或复用 Worker → 编码不可变 bundle → 校验 runtime → 激活（promote）。部署状态位于本机 SQLite；`ocd` 监督当前 workerd 进程。`oc run` 使用本地路径；`oc deploy` 使用远端 HTTPS。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

校验失败不改变当前 active。promotion / rollback 只切换 active 指针，不修改已 ready 的部署内容。预览即为 `oc run` 打印的本机 URL。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 版本不可变；发布切换 active 指针 | 是，见 [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | 是 |
| 回滚指向旧版本，而不是改字节 | 是 | 是 |
| 部署记录 | Cloudflare 全球 rollout / placement / traffic-splitting | 本机 SQLite；`ocd` 监督当前 workerd 进程 |
| gradual deployments / version affinity / Cloudflare preview URL / Workers Builds CI | 是 | 不提供 |
| `oc run` origin | 不适用 | loopback HTTP（或显式配置的本地 origin） |
| `oc deploy` | Wrangler deploy | 只接受 HTTPS，不接受带凭据的 URL |

