# Versions and deployments

一次部署：创建或复用 Worker → 编码不可变 bundle → 校验 runtime → 激活（promote）。权威是本节点 SQLite 和一个受监督的 runtime generation。`oc run` 走本地路径；`oc deploy` 走远端 HTTPS。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

失败的校验不改变当前 active。promotion / rollback 改的是 active 指针，不改已经 ready 的部署内容。预览就是 `oc run` 打印的本节点 URL。

## 兼容性

| 主题 | Cloudflare | open-compute |
| --- | --- | --- |
| 版本不可变；发布切换 active 指针 | 是，见 [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | 是 |
| 回滚指向旧版本，而不是改字节 | 是 | 是 |
| 部署权威 | Cloudflare 全球 rollout / placement / traffic-splitting | 本节点 SQLite 与一份受监督的 runtime generation |
| gradual deployments / version affinity / Cloudflare preview URL / Workers Builds CI | 是 | 不提供 |
| `oc run` origin | 不适用 | loopback HTTP（或显式配置的本地 origin） |
| `oc deploy` | Wrangler deploy | 只接受 HTTPS，不接受带凭据的 URL |

