# Versions and deployments

一次部署：创建或复用 Worker → 编码不可变 bundle → 校验 runtime → 激活（promote）。权威是本机 SQLite 和一个受监督的 runtime generation。`oc run` 做本地这条路径；`oc deploy` 走远端 HTTPS。

```sh
bun run oc run --config examples/hello-worker/open-compute.json --ocd "$PWD/target/debug/ocd"
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

失败的校验不改变当前 active。promotion / rollback 改的是 active 指针，不改已经 ready 的部署内容。预览就是 `oc run` 打印的本机 URL。

## 与 Cloudflare 相同

版本是不可变的；发布是切换 active。回滚指向旧版本，而不是改字节。对照 [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/)。

## 故意不同：OC-DEPLOY-001

Deployments、routes、promotion 和 rollback 使用一个本地 SQLite 权威和一个受监督的 runtime generation。平台不声称 Cloudflare 的全球 rollout、placement、traffic-splitting、account-management 或 billing control planes。

没有 gradual deployments、version affinity、Cloudflare preview URLs、Workers Builds CI。`run` 要求 loopback HTTP（或你显式配置的本地 origin）；远端 `deploy` 只接受 HTTPS，不接受带凭据的 URL。
