---
title: "Versions and deployments"
---

一次部署：创建或复用 Worker → 编码不可变 bundle → 校验 runtime → 激活（promote）。部署状态位于本机 SQLite；`ocd` 监督当前 workerd 进程。所选 local instance 和显式 remote target 使用同一个固定上游 Wrangler wire path。

```sh
ocd wrangler --project examples/hello-worker deploy --env dev
# Worker is serving at http://127.0.0.1:8787/<path>
# Deployment: <deployment-id>
```

校验失败不改变当前 active。deploy / rollback 只切换 active pointer，不修改已 ready Version 的 bytes。

## 兼容性

| 主题                                                                                | Cloudflare                                                                                           | open-compute                               |
| ----------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------ |
| 版本不可变；发布切换 active 指针                                                    | 是，见 [Versions & deployments](https://developers.cloudflare.com/workers/versions-and-deployments/) | 是                                         |
| 回滚指向旧版本，而不是改字节                                                        | 是                                                                                                   | 是                                         |
| 部署记录                                                                            | Cloudflare 全球 rollout / placement / traffic-splitting                                              | 本机 SQLite；`ocd` 监督当前 workerd 进程   |
| gradual deployments / version affinity / Cloudflare preview URL / Workers Builds CI | 是                                                                                                   | 不提供                                     |
| `ocd wrangler` local target                                                         | 不适用                                                                                               | 已验证的 local instance admin API          |
| `ocd wrangler --target`                                                             | Wrangler deploy                                                                                      | 显式 HTTPS target；只有 loopback 可用 HTTP |
